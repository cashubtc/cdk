use std::collections::HashMap;
use std::str::FromStr;

use anyhow::{bail, Result};
use trust_dns_resolver::config::{ResolverConfig, ResolverOpts};
use trust_dns_resolver::TokioAsyncResolver;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bip353Address {
    pub user: String,
    pub domain: String,
}

impl Bip353Address {
    /// Resolve a human-readable Bitcoin address
    pub async fn resolve(self) -> Result<PaymentInstruction> {
        // Construct DNS name
        let dns_name = format!("{}.user._bitcoin-payment.{}", self.user, self.domain);

        // Create a new resolver with DNSSEC validation
        let mut opts = ResolverOpts::default();
        opts.validate = true; // Enable DNSSEC validation

        let resolver = TokioAsyncResolver::tokio(ResolverConfig::default(), opts);

        // Query TXT records - with opts.validate=true, this will fail if DNSSEC validation fails
        let response = resolver.txt_lookup(&dns_name).await?;

        // Extract and concatenate TXT record strings
        let mut bitcoin_uris = Vec::new();

        for txt in response.iter() {
            let txt_data: Vec<String> = txt
                .txt_data()
                .iter()
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
                .collect();

            let concatenated = txt_data.join("");

            if concatenated.to_lowercase().starts_with("bitcoin:") {
                bitcoin_uris.push(concatenated);
            }
        }

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

    /// Parse a human-readable Bitcoin address
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

/// Payment instruction type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PaymentType {
    OnChain,
    LightningOffer,
}

/// BIP-353 payment instruction
#[derive(Debug, Clone)]
pub struct PaymentInstruction {
    pub parameters: HashMap<PaymentType, String>,
}

impl PaymentInstruction {
    /// Parse a payment instruction from a Bitcoin URI
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
                    let payment_type;
                    // Determine payment type
                    if key.contains("lno") {
                        payment_type = PaymentType::LightningOffer;
                    } else if !uri[8..].contains('?') && uri.len() > 8 {
                        // Simple on-chain address
                        payment_type = PaymentType::OnChain;
                    } else {
                        continue;
                    }

                    parameters.insert(payment_type, value);
                }
            }
        }

        Ok(PaymentInstruction { parameters })
    }
}
