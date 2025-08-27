#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

pub mod amount;
pub mod dhke;
pub mod mint_url;
pub mod nuts;
pub mod secret;
pub mod util;

pub use lightning_invoice::{self, Bolt11Invoice};

pub use self::amount::Amount;
pub use self::mint_url::MintUrl;
pub use self::nuts::*;
pub use self::util::SECP256K1;

#[doc(hidden)]
#[macro_export]
macro_rules! ensure_cdk {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err($err);
        }
    };
}

#[cfg(feature = "mint")]
/// Quote ID. The specifications only define a string but CDK uses Uuid, so we use an enum to port compatibility.
pub mod quote_id {
    use std::fmt;
    use std::str::FromStr;

    use bitcoin::base64::engine::general_purpose;
    use bitcoin::base64::Engine as _;
    use serde::{de, Deserialize, Deserializer, Serialize};
    use thiserror::Error;
    use uuid::Uuid;

    /// Invalid UUID
    #[derive(Debug, Error)]
    pub enum QuoteIdError {
        /// UUID Error
        #[error("invalid UUID: {0}")]
        Uuid(#[from] uuid::Error),
        /// Invalid base64
        #[error("invalid base64")]
        Base64,
        /// Invalid quote ID
        #[error("neither a valid UUID nor a valid base64 string")]
        InvalidQuoteId,
    }

    /// Mint Quote ID
    #[derive(Serialize, Debug, Clone, PartialEq, PartialOrd, Ord, Eq, Hash)]
    #[serde(untagged)]
    pub enum QuoteId {
        /// (Nutshell) base64 quote ID
        BASE64(String),
        /// UUID quote ID
        UUID(Uuid),
    }

    impl QuoteId {
        /// Create a new UUID-based MintQuoteId
        pub fn new_uuid() -> Self {
            Self::UUID(Uuid::new_v4())
        }
    }

    impl From<Uuid> for QuoteId {
        fn from(uuid: Uuid) -> Self {
            Self::UUID(uuid)
        }
    }

    impl fmt::Display for QuoteId {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                QuoteId::BASE64(s) => write!(f, "{}", s),
                QuoteId::UUID(u) => write!(f, "{}", u),
            }
        }
    }

    impl FromStr for QuoteId {
        type Err = QuoteIdError;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            // Try UUID first
            if let Ok(u) = Uuid::parse_str(s) {
                return Ok(QuoteId::UUID(u));
            }

            // Try base64: decode, then re-encode and compare to ensure canonical form
            // Use the standard (URL/filename safe or standard) depending on your needed alphabet.
            // Here we use standard base64.
            match general_purpose::URL_SAFE.decode(s) {
                Ok(_bytes) => Ok(QuoteId::BASE64(s.to_string())),
                Err(_) => Err(QuoteIdError::InvalidQuoteId),
            }
        }
    }

    impl<'de> Deserialize<'de> for QuoteId {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            // Deserialize as plain string first
            let s = String::deserialize(deserializer)?;

            // Try UUID first
            if let Ok(u) = Uuid::parse_str(&s) {
                return Ok(QuoteId::UUID(u));
            }

            if general_purpose::URL_SAFE.decode(&s).is_ok() {
                return Ok(QuoteId::BASE64(s));
            }

            // Neither matched — return a helpful error
            Err(de::Error::custom(format!(
                "QuoteId must be either a UUID (e.g. {}) or a valid base64 string; got: {}",
                Uuid::nil(),
                s
            )))
        }
    }
}
