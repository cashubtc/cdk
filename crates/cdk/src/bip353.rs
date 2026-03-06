//! BIP-353: Human Readable Bitcoin Payment Instructions
//!
//! This module provides functionality for resolving human-readable Bitcoin addresses
//! according to BIP-353. It allows users to share simple email-like addresses such as
//! `user@domain.com` instead of complex Bitcoin payment instructions.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};

use crate::wallet::MintConnector;

/// BIP-353 human-readable Bitcoin address
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bip353Address {
    /// The user part of the address (before @)
    pub user: String,
    /// The domain part of the address (after @)
    pub domain: String,
}

impl Bip353Address {
    /// Resolve a human-readable Bitcoin address to a concrete Bitcoin URI.
    ///
    /// This method performs the following steps:
    /// 1. Constructs the DNS name according to BIP-353 format
    /// 2. Queries TXT records with DNSSEC validation
    /// 3. Extracts Bitcoin URIs from the records
    /// 4. Returns the single resolved URI for downstream parsing
    ///
    /// # Errors
    ///
    /// This method will return an error if:
    /// - DNS resolution fails
    /// - DNSSEC validation fails
    /// - No Bitcoin URI is found
    /// - Multiple Bitcoin URIs are found (BIP-353 requires exactly one)
    pub(crate) async fn resolve(
        self,
        client: &Arc<dyn MintConnector + Send + Sync>,
    ) -> Result<String> {
        let dns_name = format!("{}.user._bitcoin-payment.{}", self.user, self.domain);

        let bitcoin_uris = client
            .resolve_dns_txt(&dns_name)
            .await?
            .into_iter()
            .filter(|txt_data| txt_data.to_lowercase().starts_with("bitcoin:"))
            .collect::<Vec<_>>();

        match bitcoin_uris.len() {
            0 => bail!("No Bitcoin URI found"),
            1 => Ok(bitcoin_uris[0].clone()),
            _ => bail!("Multiple Bitcoin URIs found"),
        }
    }
}

impl FromStr for Bip353Address {
    type Err = anyhow::Error;

    /// Parse a human-readable Bitcoin address from string format
    ///
    /// Accepts formats:
    /// - `user@domain.com`
    /// - `₿user@domain.com` (with Bitcoin symbol prefix)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The format is not `user@domain`
    /// - User or domain parts are empty
    fn from_str(address: &str) -> Result<Self, Self::Err> {
        let addr = address.trim();
        let addr = addr.strip_prefix("₿").unwrap_or(addr);

        let parts: Vec<&str> = addr.split('@').collect();
        if parts.len() != 2 {
            bail!("Address is not formatted correctly")
        }

        let user = parts[0].trim();
        let domain = parts[1].trim();

        if user.is_empty() || domain.is_empty() {
            bail!("User name and domain must not be empty")
        }

        Ok(Self {
            user: user.to_string(),
            domain: domain.to_string(),
        })
    }
}

impl std::fmt::Display for Bip353Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.user, self.domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bip353_address_parsing() {
        let addr = Bip353Address::from_str("alice@example.com").unwrap();
        assert_eq!(addr.user, "alice");
        assert_eq!(addr.domain, "example.com");

        let addr = Bip353Address::from_str("₿bob@bitcoin.org").unwrap();
        assert_eq!(addr.user, "bob");
        assert_eq!(addr.domain, "bitcoin.org");

        let addr = Bip353Address::from_str("  charlie@test.net  ").unwrap();
        assert_eq!(addr.user, "charlie");
        assert_eq!(addr.domain, "test.net");

        let addr = Bip353Address {
            user: "test".to_string(),
            domain: "example.com".to_string(),
        };
        assert_eq!(addr.to_string(), "test@example.com");
    }

    #[test]
    fn test_bip353_address_parsing_errors() {
        assert!(Bip353Address::from_str("invalid").is_err());
        assert!(Bip353Address::from_str("@example.com").is_err());
        assert!(Bip353Address::from_str("user@").is_err());
        assert!(Bip353Address::from_str("user@domain@extra").is_err());
        assert!(Bip353Address::from_str("").is_err());
    }
}
