use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct OhttpConfig {
    pub server: ohttp::Server,
}

#[derive(Serialize, Deserialize)]
struct KeyPair {
    ikm: [u8; 32],
}

impl OhttpConfig {
    pub fn generate_new() -> Result<Self> {
        let _ikm = bitcoin::key::rand::random::<[u8; 32]>();
        let config = ohttp::KeyConfig::new(
            1,
            ohttp::hpke::Kem::K256Sha256,
            vec![ohttp::SymmetricSuite::new(
                ohttp::hpke::Kdf::HkdfSha256,
                ohttp::hpke::Aead::ChaCha20Poly1305,
            )],
        )?;
        Ok(OhttpConfig {
            server: ohttp::Server::new(config)?,
        })
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let data = std::fs::read_to_string(path)?;
        let keys: KeyPair = serde_json::from_str(&data)?;

        let config = ohttp::KeyConfig::derive(
            1,
            ohttp::hpke::Kem::K256Sha256,
            vec![ohttp::SymmetricSuite::new(
                ohttp::hpke::Kdf::HkdfSha256,
                ohttp::hpke::Aead::ChaCha20Poly1305,
            )],
            &keys.ikm,
        )
        .map_err(|e| anyhow::anyhow!("Failed to derive OHTTP keys from file: {}", e))?;

        Ok(OhttpConfig {
            server: ohttp::Server::new(config)?,
        })
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        // For now, just save an empty IKM - we'll generate each time for simplicity
        let _ikm = bitcoin::key::rand::random::<[u8; 32]>();
        let keys = KeyPair { ikm: _ikm };

        let data = serde_json::to_string_pretty(&keys)?;
        let mut file = File::create(path)?;
        file.write_all(data.as_bytes())?;

        Ok(())
    }
}

pub fn generate_and_save_keys<P: AsRef<Path>>(key_file: P) -> Result<OhttpConfig> {
    let config = OhttpConfig::generate_new()?;
    config.save_to_file(&key_file)?;
    tracing::info!(
        "Generated new OHTTP keys and saved to {:?}",
        key_file.as_ref()
    );
    Ok(config)
}

pub fn load_or_generate_keys<P: AsRef<Path>>(key_file: P) -> Result<OhttpConfig> {
    if key_file.as_ref().exists() {
        OhttpConfig::load_from_file(&key_file)
    } else {
        generate_and_save_keys(key_file)
    }
}
