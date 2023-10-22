// MIT License
// Copyright (c) 2023 Clark Moody
// https://github.com/clarkmoody/cashu-rs/blob/master/src/secret.rs

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The secret data that allows spending ecash
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

#[derive(Debug)]
pub enum Error {
    InvalidLength(u64),
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    const BIT_LENGTH: usize = 128;
    /// Create secret value
    pub fn new() -> Self {
        use base64::engine::general_purpose::URL_SAFE;
        use base64::Engine as _;
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

impl FromStr for Secret {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len().ne(&24) {
            return Err(Error::InvalidLength(s.as_bytes().len() as u64));
        }

        Ok(Secret(s.to_string()))
    }
}

impl ToString for Secret {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_secret_from_str() {
        let secret = Secret::new();

        let secret_str = secret.to_string();

        let secret_n = Secret::from_str(&secret_str).unwrap();

        assert_eq!(secret_n, secret)
    }
}
