//! SQLCipher database key

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// A SQLCipher database key.
///
/// The key is never empty. `Debug` never prints it.
#[derive(Clone)]
pub struct SqlcipherKey(String);

impl SqlcipherKey {
    /// The key as passed to SQLCipher.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SqlcipherKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SqlcipherKey([REDACTED])")
    }
}

/// The text cannot be used as a SQLCipher key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SqlcipherKeyError {
    /// The key is empty.
    #[error("SQLCipher key must not be empty")]
    Empty,
}

impl FromStr for SqlcipherKey {
    type Err = SqlcipherKeyError;

    fn from_str(key: &str) -> Result<Self, Self::Err> {
        if key.is_empty() {
            return Err(SqlcipherKeyError::Empty);
        }

        Ok(Self(key.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::{SqlcipherKey, SqlcipherKeyError};

    #[test]
    fn empty_key_is_rejected() {
        assert_eq!(
            "".parse::<SqlcipherKey>().map(|_| ()),
            Err(SqlcipherKeyError::Empty)
        );
    }

    #[test]
    fn debug_redacts_key() {
        let secret = "sqlcipher-key-secret";
        let key: SqlcipherKey = secret.parse().expect("non-empty key");

        let debug = format!("{key:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(secret));
    }
}
