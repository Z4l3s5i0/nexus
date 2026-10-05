use tokio::io::{AsyncReadExt, AsyncWriteExt};
use anyhow::{Result, anyhow};
use k256::{SecretKey, PublicKey};
use alloy_primitives::B256;
use crate::rlpx::stream::RlpxStream;
use crate::rlpx::crypto::{AuthMsgV4, AuthAckV4, b512_to_pubkey, pubkey_to_b512, derive_session_secrets, ecies_encrypt, ecies_decrypt, recover_pubkey};
use alloy_rlp::Decodable;
use wasix_eth_utils::debug;
use crate::rlpx::frame::RlpxFrameCodec;
use rand::Rng;
use k256::elliptic_curve::rand_core::OsRng;

pub async fn do_handshake<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut RlpxStream<S>,
    initiator: bool,
    local_sk: &SecretKey,
    remote_pk: Option<&PublicKey>,
) -> Result<()> {
    let initiator_session_eph_sk = SecretKey::random(&mut OsRng);
    let initiator_nonce = B256::from(rand::random::<[u8; 32]>());

    if initiator {
        debug!("[ECIES] Initiator: preparing auth message");

        let remote_pk = remote_pk.ok_or_else(|| anyhow!("Remote public key required for initiator"))?;

        let static_shared_secret = k256::elliptic_curve::ecdh::diffie_hellman(
            local_sk.to_nonzero_scalar(),
            remote_pk.as_affine(),
        );
        let static_shared_secret_bytes = static_shared_secret.raw_secret_bytes();

        let mut msg_to_sign = [0u8; 32];
        for i in 0..32 {
            msg_to_sign[i] = static_shared_secret_bytes[i] ^ initiator_nonce[i];
        }

        let signing_key = k256::ecdsa::SigningKey::from(&initiator_session_eph_sk);
        tokio::task::yield_now().await;
        let (signature, recovery_id) = signing_key.sign_prehash_recoverable(&msg_to_sign)
            .map_err(|e| anyhow!("Signing failed: {}", e))?;
        tokio::task::yield_now().await;

        let mut sig_bytes = [0u8; 65];
        sig_bytes[0..64].copy_from_slice(&signature.to_bytes());
        sig_bytes[64] = recovery_id.to_byte();

        let auth_msg = AuthMsgV4 {
            signature: sig_bytes,
            initiator_pubkey: pubkey_to_b512(&local_sk.public_key()),
            nonce: initiator_nonce,
            version: 4,
        };

        let auth_rlp = alloy_rlp::encode(&auth_msg);
        let mut auth_body = auth_rlp.to_vec();
        {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            auth_body.resize(auth_body.len() + rng.gen_range(100..300), 0);
        }
        let full_auth = ecies_encrypt(remote_pk, &auth_body, &initiator_session_eph_sk)?;
        debug!("[ECIES] Initiator: writing auth packet, len={}", full_auth.len());
        stream.inner.write_all(&full_auth).await?;
        debug!("[ECIES] Initiator: auth packet written, waiting for ack length");

        let mut ack_len_buf = [0u8; 2];
        stream.inner.read_exact(&mut ack_len_buf).await?;
        let ack_len = u16::from_be_bytes(ack_len_buf) as usize;
        debug!("[ECIES] Initiator: received ack length {}", ack_len);

        let mut encrypted_ack = vec![0u8; ack_len];
        stream.inner.read_exact(&mut encrypted_ack).await?;
        debug!("[ECIES] Initiator: received encrypted ack");

        let mut full_ack = Vec::with_capacity(2 + ack_len);
        full_ack.extend_from_slice(&ack_len_buf);
        full_ack.extend_from_slice(&encrypted_ack);

        let ack_rlp = ecies_decrypt(local_sk, &encrypted_ack, &ack_len_buf)?;
        tokio::task::yield_now().await;
        let mut ack_rlp_slice = &ack_rlp[..];
        let ack_msg = AuthAckV4::decode(&mut ack_rlp_slice)?;

        let recipient_session_eph_pk = b512_to_pubkey(&ack_msg.recipient_ephemeral_pubkey)?;
        let recipient_nonce = ack_msg.nonce;

        let agreed_secret = k256::elliptic_curve::ecdh::diffie_hellman(
            initiator_session_eph_sk.to_nonzero_scalar(),
            recipient_session_eph_pk.as_affine(),
        );
        let agreed_secret_bytes: [u8; 32] = (*agreed_secret.raw_secret_bytes()).into();

        let secrets = derive_session_secrets(
            true,
            &initiator_nonce,
            &recipient_nonce,
            &agreed_secret_bytes,
            &full_auth,
            &full_ack,
        );

        stream.secrets = Some(secrets);
        stream.codec = Some(RlpxFrameCodec::new(stream.secrets.as_ref().unwrap()));
        stream.remote_id = Some(pubkey_to_b512(remote_pk));
        debug!("[ECIES] Initiator: handshake complete");

    } else {
        debug!("[ECIES] Recipient: waiting for auth length");

        let mut auth_len_buf = [0u8; 2];
        stream.inner.read_exact(&mut auth_len_buf).await?;
        let auth_len = u16::from_be_bytes(auth_len_buf) as usize;
        debug!("[ECIES] Recipient: received auth length {}", auth_len);

        let mut encrypted_auth = vec![0u8; auth_len];
        stream.inner.read_exact(&mut encrypted_auth).await?;
        debug!("[ECIES] Recipient: received encrypted auth");

        let mut full_auth = Vec::with_capacity(2 + auth_len);
        full_auth.extend_from_slice(&auth_len_buf);
        full_auth.extend_from_slice(&encrypted_auth);

        let auth_rlp = ecies_decrypt(local_sk, &encrypted_auth, &auth_len_buf)?;
        tokio::task::yield_now().await;
        let mut auth_rlp_slice = &auth_rlp[..];
        let auth_msg = AuthMsgV4::decode(&mut auth_rlp_slice)?;

        let initiator_static_pk = b512_to_pubkey(&auth_msg.initiator_pubkey)?;
        let initiator_nonce = auth_msg.nonce;

        let static_shared_secret = k256::elliptic_curve::ecdh::diffie_hellman(
            local_sk.to_nonzero_scalar(),
            initiator_static_pk.as_affine(),
        );
        let static_shared_secret_bytes = static_shared_secret.raw_secret_bytes();

        let mut msg_to_sign = [0u8; 32];
        for i in 0..32 {
            msg_to_sign[i] = static_shared_secret_bytes[i] ^ initiator_nonce[i];
        }

        let initiator_session_eph_pk = recover_pubkey(&auth_msg.signature, &msg_to_sign)?;
        tokio::task::yield_now().await;

        let recipient_session_eph_sk = SecretKey::random(&mut OsRng);
        let recipient_nonce = B256::from(rand::random::<[u8; 32]>());

        let ack_msg = AuthAckV4 {
            recipient_ephemeral_pubkey: pubkey_to_b512(&recipient_session_eph_sk.public_key()),
            nonce: recipient_nonce,
            version: 4,
        };

        let ack_rlp = alloy_rlp::encode(&ack_msg);
        let mut ack_body = ack_rlp.to_vec();
        {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            ack_body.resize(ack_body.len() + rng.gen_range(100..300), 0);
        }
        let full_ack = ecies_encrypt(&initiator_static_pk, &ack_body, &recipient_session_eph_sk)?;
        debug!("[ECIES] Recipient: writing ack packet, len={}", full_ack.len());
        stream.inner.write_all(&full_ack).await?;
        debug!("[ECIES] Recipient: ack packet written");

        let agreed_secret = k256::elliptic_curve::ecdh::diffie_hellman(
            recipient_session_eph_sk.to_nonzero_scalar(),
            initiator_session_eph_pk.as_affine(),
        );
        let agreed_secret_bytes: [u8; 32] = (*agreed_secret.raw_secret_bytes()).into();

        let secrets = derive_session_secrets(
            false,
            &initiator_nonce,
            &recipient_nonce,
            &agreed_secret_bytes,
            &full_auth,
            &full_ack,
        );

        stream.secrets = Some(secrets);
        stream.codec = Some(RlpxFrameCodec::new(stream.secrets.as_ref().unwrap()));
        stream.remote_id = Some(auth_msg.initiator_pubkey);
        debug!("[ECIES] Recipient: handshake complete");

    }
    Ok(())
}
