// MIT License
// Copyright (c) 2023 Clark Moody
// https://github.com/clarkmoody/cashu-rs/blob/master/src/secret.rs

use serde::{Deserialize, Serialize};

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    const BIT_LENGTH: usize = 128;

    /// Create secret value
    pub fn new() -> Self {
        use base64::{engine::general_purpose::URL_SAFE, Engine as _};
        use rand::RngCore;

        let mut rng = rand::thread_rng();

        let mut random_bytes = [0u8; Self::BIT_LENGTH / 8];

        // Generate random bytes
        rng.fill_bytes(&mut random_bytes);
        // The secret string is Base64-encoded
        let secret = URL_SAFE.encode(random_bytes);
        Self(secret)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
