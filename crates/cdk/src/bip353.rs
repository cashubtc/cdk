//! BIP-353: Human Readable Bitcoin Payment Instructions
//!
//! This module provides functionality for resolving human-readable Bitcoin addresses
//! according to BIP-353. It allows users to share simple email-like addresses such as
//! `user@domain.com` instead of complex Bitcoin addresses or Lightning invoices.

use std::collections::HashMap;
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
    /// Resolve a human-readable Bitcoin address to payment instructions
    ///
    /// This method performs the following steps:
    /// 1. Constructs the DNS name according to BIP-353 format
    /// 2. Queries TXT records with DNSSEC validation
    /// 3. Extracts Bitcoin URIs from the records
    /// 4. Parses the URIs into payment instructions
    ///
    /// # Errors
    ///
    /// This method will return an error if:
    /// - DNS resolution fails
    /// - DNSSEC validation fails
    /// - No Bitcoin URI is found
    /// - Multiple Bitcoin URIs are found (BIP-353 requires exactly one)
    /// - The URI format is invalid
    pub(crate) async fn resolve(
        self,
        client: &Arc<dyn MintConnector + Send + Sync>,
    ) -> Result<PaymentInstruction> {
        // Construct DNS name according to BIP-353
        let dns_name = format!("{}.user._bitcoin-payment.{}", self.user, self.domain);

        let bitcoin_uris = client
            .resolve_dns_txt(&dns_name)
            .await?
            .into_iter()
            .filter(|txt_data| txt_data.to_lowercase().starts_with("bitcoin:"))
            .collect::<Vec<_>>();

        // BIP-353 requires exactly one Bitcoin URI
        match bitcoin_uris.len() {
            0 => bail!("No Bitcoin URI found"),
            1 => PaymentInstruction::from_uri(&bitcoin_uris[0]),
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

        // Remove Bitcoin prefix if present
        let addr = addr.strip_prefix("₿").unwrap_or(addr);

        // Split by @
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

/// Payment instruction type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaymentType {
    /// On-chain Bitcoin address
    OnChain,
    /// Lightning Offer (BOLT12)
    LightningOffer,
}

/// BIP-353 payment instruction containing parsed payment methods
#[derive(Debug, Clone)]
pub struct PaymentInstruction {
    /// Map of payment types to their corresponding values
    pub parameters: HashMap<PaymentType, String>,
}

impl PaymentInstruction {
    /// Create a new empty payment instruction
    pub fn new() -> Self {
        Self {
            parameters: HashMap::new(),
        }
    }

    /// Parse a payment instruction from a Bitcoin URI
    ///
    /// Extracts various payment methods from the URI:
    /// - Lightning offers (parameters containing "lno")
    /// - On-chain addresses (address part of the URI)
    ///
    /// # Errors
    ///
    /// Returns an error if the URI doesn't start with "bitcoin:"
    pub fn from_uri(uri: &str) -> Result<Self> {
        if !uri.to_lowercase().starts_with("bitcoin:") {
            bail!("URI must start with 'bitcoin:'")
        }

        let mut parameters = HashMap::new();

        // Parse URI parameters
        if let Some(query_start) = uri.find('?') {
            let query = &uri[query_start + 1..];
            for pair in query.split('&') {
                if let Some(eq_pos) = pair.find('=') {
                    let key = pair[..eq_pos].to_string();
                    let value = pair[eq_pos + 1..].to_string();

                    // Determine payment type based on parameter key
                    if key.contains("lno") {
                        parameters.insert(PaymentType::LightningOffer, value);
                    }
                    // Could add more payment types here as needed
                }
            }
        }

        // Check if we have an on-chain address (address part after bitcoin:)
        if let Some(query_start) = uri.find('?') {
            let addr_part = &uri[8..query_start]; // Skip "bitcoin:"
            if !addr_part.is_empty() {
                parameters.insert(PaymentType::OnChain, addr_part.to_string());
            }
        } else {
            // No query parameters, check if there's just an address
            let addr_part = &uri[8..]; // Skip "bitcoin:"
            if !addr_part.is_empty() {
                parameters.insert(PaymentType::OnChain, addr_part.to_string());
            }
        }

        Ok(PaymentInstruction { parameters })
    }

    /// Get a payment method by type
    pub fn get(&self, payment_type: &PaymentType) -> Option<&String> {
        self.parameters.get(payment_type)
    }
}

impl Default for PaymentInstruction {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl PaymentInstruction {
        /// Check if a payment type is available
        pub fn has_payment_type(&self, payment_type: &PaymentType) -> bool {
            self.parameters.contains_key(payment_type)
        }
    }

    #[test]
    fn test_bip353_address_parsing() {
        // Test basic parsing
        let addr = Bip353Address::from_str("alice@example.com").unwrap();
        assert_eq!(addr.user, "alice");
        assert_eq!(addr.domain, "example.com");

        // Test with Bitcoin symbol
        let addr = Bip353Address::from_str("₿bob@bitcoin.org").unwrap();
        assert_eq!(addr.user, "bob");
        assert_eq!(addr.domain, "bitcoin.org");

        // Test with whitespace
        let addr = Bip353Address::from_str("  charlie@test.net  ").unwrap();
        assert_eq!(addr.user, "charlie");
        assert_eq!(addr.domain, "test.net");

        // Test display
        let addr = Bip353Address {
            user: "test".to_string(),
            domain: "example.com".to_string(),
        };
        assert_eq!(addr.to_string(), "test@example.com");
    }

    #[test]
    fn test_bip353_address_parsing_errors() {
        // Test invalid formats
        assert!(Bip353Address::from_str("invalid").is_err());
        assert!(Bip353Address::from_str("@example.com").is_err());
        assert!(Bip353Address::from_str("user@").is_err());
        assert!(Bip353Address::from_str("user@domain@extra").is_err());
        assert!(Bip353Address::from_str("").is_err());
    }

    #[test]
    fn test_payment_instruction_parsing() {
        // Test Lightning offer URI
        let uri = "bitcoin:?lno=lno1qcp4256ypqpq86q2pucnq42ngssx2an9wfujqerp0y2pxqrjszs5v2a5m5xwc4mxv6rdjdcn2d3kxccnjdgecf7fz3rf5g4t7gdxhkzm8mpsq5q";
        let instruction = PaymentInstruction::from_uri(uri).unwrap();
        assert!(instruction.has_payment_type(&PaymentType::LightningOffer));

        // Test on-chain address URI
        let uri = "bitcoin:bc1qexampleaddress";
        let instruction = PaymentInstruction::from_uri(uri).unwrap();
        assert!(instruction.has_payment_type(&PaymentType::OnChain));
        assert_eq!(
            instruction.get(&PaymentType::OnChain).unwrap(),
            "bc1qexampleaddress"
        );

        // Test combined URI
        let uri = "bitcoin:bc1qexampleaddress?lno=lno1qcp4256ypqpq86q2pucnq42ngssx2an9wfujqerp0y2pxqrjszs5v2a5m5xwc4mxv6rdjdcn2d3kxccnjdgecf7fz3rf5g4t7gdxhkzm8mpsq5q";
        let instruction = PaymentInstruction::from_uri(uri).unwrap();
        assert!(instruction.has_payment_type(&PaymentType::OnChain));
        assert!(instruction.has_payment_type(&PaymentType::LightningOffer));
    }

    #[test]
    fn test_payment_instruction_errors() {
        // Test invalid URI
        assert!(PaymentInstruction::from_uri("invalid:uri").is_err());
        assert!(PaymentInstruction::from_uri("").is_err());
    }
}
