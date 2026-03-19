//! BIP-353: Human Readable Bitcoin Payment Instructions
//!
//! This module provides functionality for resolving human-readable Bitcoin addresses
//! according to BIP-353. It allows users to share simple email-like addresses such as
//! `user@domain.com` instead of complex Bitcoin payment instructions.

use core::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::wallet::MintConnector;

/// Errors that can occur when parsing or resolving a BIP-353 address.
#[derive(Debug)]
pub enum Bip353Error {
    /// The address string is not in a valid `user@domain` format.
    InvalidFormat,
    /// The user or domain part of the address is empty.
    EmptyUserOrDomain,
    /// DNS resolution returned no `bitcoin:` URI.
    NoBitcoinUri,
    /// DNS resolution returned more than one `bitcoin:` URI.
    MultipleBitcoinUris,
    /// DNS resolution or network I/O failed.
    DnsResolution(String),
}

impl fmt::Display for Bip353Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => write!(f, "Address is not formatted correctly"),
            Self::EmptyUserOrDomain => {
                write!(f, "User name and domain must not be empty")
            }
            Self::NoBitcoinUri => write!(f, "No Bitcoin URI found"),
            Self::MultipleBitcoinUris => write!(f, "Multiple Bitcoin URIs found"),
            Self::DnsResolution(e) => write!(f, "DNS resolution failed: {e}"),
        }
    }
}

impl std::error::Error for Bip353Error {}

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
    ) -> Result<String, Bip353Error> {
        let dns_name = format!("{}.user._bitcoin-payment.{}", self.user, self.domain);

        let mut bitcoin_uris: Vec<String> = client
            .resolve_dns_txt(&dns_name)
            .await
            .map_err(|e| Bip353Error::DnsResolution(e.to_string()))?
            .into_iter()
            .filter(|txt_data| {
                txt_data
                    .get(..8)
                    .map(|p| p.eq_ignore_ascii_case("bitcoin:"))
                    .unwrap_or(false)
            })
            .collect();

        match bitcoin_uris.len() {
            0 => Err(Bip353Error::NoBitcoinUri),
            1 => Ok(bitcoin_uris.swap_remove(0)),
            _ => Err(Bip353Error::MultipleBitcoinUris),
        }
    }
}

impl FromStr for Bip353Address {
    type Err = Bip353Error;

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
            return Err(Bip353Error::InvalidFormat);
        }

        let user = parts[0].trim();
        let domain = parts[1].trim();

        if user.is_empty() || domain.is_empty() {
            return Err(Bip353Error::EmptyUserOrDomain);
        }

        Ok(Self {
            user: user.to_string(),
            domain: domain.to_string(),
        })
    }
}

impl fmt::Display for Bip353Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

    #[test]
    fn test_bip353_address_with_subdomain() {
        let addr = Bip353Address::from_str("alice@sub.domain.co.uk").unwrap();
        assert_eq!(addr.user, "alice");
        assert_eq!(addr.domain, "sub.domain.co.uk");
        assert_eq!(addr.to_string(), "alice@sub.domain.co.uk");
    }
}
