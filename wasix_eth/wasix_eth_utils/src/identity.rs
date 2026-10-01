use std::fs;
use std::path::Path;
use anyhow::{Context, Result};
use k256::ecdsa::SigningKey;
use k256::SecretKey;
use wasix_eth_types::hex;
use k256::elliptic_curve::rand_core::OsRng;

#[derive(Clone)]
pub struct Identity {
    pub keypair: SigningKey,
}

impl Identity {
    pub fn new(
        data_dir: Option<&Path>,
        peer_name: Option<&str>,
    ) -> Result<Self> {
        let keypair = match data_dir {
            Some(path) => {
                let name_to_use = peer_name.unwrap_or("node");
                let key_path = path.join(format!("p2p_{}.key", name_to_use));
                
                if key_path.exists() {
                    let hex_key = fs::read_to_string(&key_path)
                        .context(format!("Failed to read key file at {:?}", key_path))?;
                    let bytes = hex::decode(hex_key.trim())
                        .context("Failed to decode hex key")?;
                    SigningKey::from_slice(&bytes)
                        .map_err(|_| anyhow::anyhow!("Invalid key bytes"))?
                } else {
                    let signing_key = SigningKey::random(&mut OsRng);

                    let hex_key = hex::encode(signing_key.to_bytes());

                    if let Some(parent) = key_path.parent() {
                        fs::create_dir_all(parent)?;
                    }

                    fs::write(&key_path, hex_key)?;

                    signing_key
                }
            }
            None => {
                SigningKey::random(&mut OsRng)
            }
        };

        Ok(Self { keypair })
    }

    /// Get the PeerId as a hex string (from public key)
    pub fn peer_id(&self) -> String {
        let pubkey = self.keypair.verifying_key();
        hex::encode(pubkey.to_sec1_bytes())
    }

    /// Get the 64-byte uncompressed public key (used for enode)
    pub fn public_key_b512(&self) -> wasix_eth_types::B512 {
        let uncompressed = self.keypair.verifying_key().to_encoded_point(false);
        let mut id_bytes = [0u8; 64];
        id_bytes.copy_from_slice(&uncompressed.as_bytes()[1..]);
        wasix_eth_types::B512::from(id_bytes)
    }

    pub fn secret_key(&self) -> SecretKey {
        SecretKey::from_slice(&self.keypair.to_bytes()).expect("Valid secret key")
    }
}
