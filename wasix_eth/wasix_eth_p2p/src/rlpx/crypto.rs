use alloy_primitives::{B256, B512};
use alloy_rlp::{Decodable, Header, RlpEncodable};
use aes::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{PublicKey, SecretKey};
use anyhow::{Result, anyhow};
use sha3::{Digest, Keccak256};
use rand::RngCore;

type HmacSha256 = Hmac<Sha256>;
type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

#[derive(RlpEncodable, Debug, Clone)]
pub struct AuthMsgV4 {
    pub signature: [u8; 65],
    pub initiator_pubkey: B512,
    pub nonce: B256,
    pub version: u64,
}

impl Decodable for AuthMsgV4 {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let header = Header::decode(buf)?;
        let mut inner = &buf[..header.payload_length];
        *buf = &buf[header.payload_length..];

        let signature = Decodable::decode(&mut inner)?;
        let initiator_pubkey = Decodable::decode(&mut inner)?;
        let nonce = Decodable::decode(&mut inner)?;
        let version = Decodable::decode(&mut inner)?;

        // EIP-8: Ignore any additional fields
        // We don't need to do anything here because Decodable for fields will consume what they need,
        // and any remaining bytes in 'inner' are just ignored by us.
        // The 'while !inner.is_empty()' was a bit overkill if we don't know the exact structure.

        Ok(Self { signature, initiator_pubkey, nonce, version })
    }
}

#[derive(RlpEncodable, Debug, Clone)]
pub struct AuthAckV4 {
    pub recipient_ephemeral_pubkey: B512,
    pub nonce: B256,
    pub version: u64,
}

impl Decodable for AuthAckV4 {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let header = Header::decode(buf)?;
        let mut inner = &buf[..header.payload_length];
        *buf = &buf[header.payload_length..];

        let recipient_ephemeral_pubkey = Decodable::decode(&mut inner)?;
        let nonce = Decodable::decode(&mut inner)?;
        let version = Decodable::decode(&mut inner)?;

        // EIP-8: Ignore any additional fields
        Ok(Self { recipient_ephemeral_pubkey, nonce, version })
    }
}

pub struct HandshakeSecrets {
    pub aes_secret: B256,
    pub mac_secret: B256,
    pub egress_mac_init_data: Vec<u8>,
    pub ingress_mac_init_data: Vec<u8>,
}

pub fn derive_session_secrets(
    initiator: bool,
    initiator_nonce: &B256,
    recipient_nonce: &B256,
    agreed_secret: &[u8; 32],
    auth_packet: &[u8],
    ack_packet: &[u8],
) -> HandshakeSecrets {
    let mut hasher = Keccak256::new();
    hasher.update(recipient_nonce.as_slice());
    hasher.update(initiator_nonce.as_slice());
    let h_nonce = B256::from_slice(&hasher.finalize());

    let mut hasher = Keccak256::new();
    hasher.update(agreed_secret);
    hasher.update(h_nonce.as_slice());
    let shared_secret = B256::from_slice(&hasher.finalize());

    let mut hasher = Keccak256::new();
    hasher.update(agreed_secret);
    hasher.update(shared_secret.as_slice());
    let aes_secret = B256::from_slice(&hasher.finalize());

    let mut hasher = Keccak256::new();
    hasher.update(agreed_secret);
    hasher.update(aes_secret.as_slice());
    let mac_secret = B256::from_slice(&hasher.finalize());

    let mut egress_mac_init_data = Vec::new();
    let mut ingress_mac_init_data = Vec::new();

    if initiator {
        let mut xored_egress = mac_secret;
        for i in 0..32 {
            xored_egress[i] ^= recipient_nonce[i];
        }
        egress_mac_init_data.extend_from_slice(xored_egress.as_slice());
        egress_mac_init_data.extend_from_slice(auth_packet);

        let mut xored_ingress = mac_secret;
        for i in 0..32 {
            xored_ingress[i] ^= initiator_nonce[i];
        }
        ingress_mac_init_data.extend_from_slice(xored_ingress.as_slice());
        ingress_mac_init_data.extend_from_slice(ack_packet);
    } else {
        let mut xored_egress = mac_secret;
        for i in 0..32 {
            xored_egress[i] ^= initiator_nonce[i];
        }
        egress_mac_init_data.extend_from_slice(xored_egress.as_slice());
        egress_mac_init_data.extend_from_slice(ack_packet);

        let mut xored_ingress = mac_secret;
        for i in 0..32 {
            xored_ingress[i] ^= recipient_nonce[i];
        }
        ingress_mac_init_data.extend_from_slice(xored_ingress.as_slice());
        ingress_mac_init_data.extend_from_slice(auth_packet);
    }

    HandshakeSecrets {
        aes_secret,
        mac_secret,
        egress_mac_init_data,
        ingress_mac_init_data,
    }
}

pub fn recover_pubkey(sig: &[u8; 65], msg: &[u8; 32]) -> Result<PublicKey> {
    let recid = k256::ecdsa::RecoveryId::try_from(sig[64] % 4)
        .map_err(|e| anyhow!("Invalid recovery ID: {}", e))?;
    let signature = k256::ecdsa::Signature::from_slice(&sig[..64])?;
    let recovered_key = k256::ecdsa::VerifyingKey::recover_from_prehash(
        msg,
        &signature,
        recid,
    ).map_err(|e| anyhow!("Key recovery failed: {}", e))?;
    Ok(PublicKey::from(&recovered_key))
}

pub fn kdf(secret: &[u8], s1: &[u8], output: &mut [u8]) {
    let mut counter = 1u32;
    let mut offset = 0;
    while offset < output.len() {
        let mut hasher = Sha256::new();
        hasher.update(&counter.to_be_bytes());
        hasher.update(secret);
        hasher.update(s1);
        let digest = hasher.finalize();
        let copy_len = std::cmp::min(digest.len(), output.len() - offset);
        output[offset..offset + copy_len].copy_from_slice(&digest[..copy_len]);
        offset += copy_len;
        counter += 1;
    }
}

pub fn ecies_encrypt(remote_pub: &PublicKey, data: &[u8], ephemeral_sk: &SecretKey) -> Result<Vec<u8>> {
    let ephemeral_pk = ephemeral_sk.public_key();
    let shared_secret = k256::elliptic_curve::ecdh::diffie_hellman(
        ephemeral_sk.to_nonzero_scalar(),
        remote_pub.as_affine(),
    );
    let shared_secret_bytes = shared_secret.raw_secret_bytes();
    
    let mut derived = [0u8; 32];
    kdf(&shared_secret_bytes, &[], &mut derived);
    
    let enc_key = &derived[0..16];
    let mac_key = Sha256::digest(&derived[16..32]);
    
    let mut iv = [0u8; 16];
    {
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut iv);
    }

    let mut cipher_text = data.to_vec();

    let total_size = (65 + 16 + cipher_text.len() + 32) as u16;
    let s2 = total_size.to_be_bytes();

    let mut aes = Aes128Ctr::new(enc_key.into(), &iv.into());
    aes.apply_keystream(&mut cipher_text);

    let mut hmac = HmacSha256::new_from_slice(&mac_key)?;
    hmac.update(&iv);
    hmac.update(&cipher_text);
    hmac.update(&s2);

    let tag = hmac.finalize().into_bytes();

    let mut result = Vec::with_capacity(2 + 65 + 16 + cipher_text.len() + 32);
    result.extend_from_slice(&s2);
    result.extend_from_slice(&ephemeral_pk.to_encoded_point(false).as_bytes());
    result.extend_from_slice(&iv);
    result.extend_from_slice(&cipher_text);
    result.extend_from_slice(&tag);
    Ok(result)
}

pub fn ecies_decrypt(local_sk: &SecretKey, data: &[u8], s2: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 65 + 16 + 32 {
        return Err(anyhow!("Data too short for ECIES"));
    }
    let ephemeral_pk = PublicKey::from_sec1_bytes(&data[0..65])?;
    let iv = &data[65..81];
    let tag = &data[data.len() - 32..];
    let cipher_text = &data[81..data.len() - 32];

    let shared_secret = k256::elliptic_curve::ecdh::diffie_hellman(
        local_sk.to_nonzero_scalar(),
        ephemeral_pk.as_affine(),
    );
    let shared_secret_bytes = shared_secret.raw_secret_bytes();

    let mut derived = [0u8; 32];
    kdf(&shared_secret_bytes, &[], &mut derived);
    
    let enc_key = &derived[0..16];
    let mac_key = Sha256::digest(&derived[16..32]);

    let mut hmac = HmacSha256::new_from_slice(&mac_key)?;
    hmac.update(iv);
    hmac.update(cipher_text);
    hmac.update(s2);

    if hmac.finalize().into_bytes().as_slice() != tag {
        return Err(anyhow!("ECIES MAC tag mismatch"));
    }

    let mut plain_text = cipher_text.to_vec();
    let mut aes = Aes128Ctr::new(enc_key.into(), iv.into());
    aes.apply_keystream(&mut plain_text);

    Ok(plain_text)
}

pub fn b512_to_pubkey(id: &B512) -> Result<PublicKey> {
    let mut bytes = [0u8; 65];
    bytes[0] = 0x04;
    bytes[1..].copy_from_slice(id.as_slice());
    PublicKey::from_sec1_bytes(&bytes).map_err(|e| anyhow!("Invalid public key: {}", e))
}

pub fn pubkey_to_b512(pubkey: &PublicKey) -> B512 {
    let encoded = pubkey.to_encoded_point(false);
    let mut id_bytes = [0u8; 64];
    id_bytes.copy_from_slice(&encoded.as_bytes()[1..]);
    B512::from(id_bytes)
}
