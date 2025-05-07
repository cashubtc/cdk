use anyhow::{bail, Result};
use std::collections::HashMap;

/// Parse a human-readable Bitcoin address
pub(crate) fn parse_address(address: &str) -> Result<(String, String)> {
    let addr = address.trim();

    // Remove Bitcoin prefix if present
    let addr = addr.strip_prefix("₿").unwrap_or(addr);

    // Split by @
    let parts: Vec<&str> = addr.split('@').collect();
    if parts.len() != 2 {
        bail!("Address is not formated correctlly")
    }

    let user = parts[0].trim();
    let domain = parts[1].trim();

    if user.is_empty() || domain.is_empty() {
        bail!("User name and domain must not be empty")
    }

    Ok((user.to_string(), domain.to_string()))
}

/// BIP-353 payment instruction
#[derive(Debug, Clone)]
pub struct PaymentInstruction {
    pub uri: String,
    pub payment_type: PaymentType,
    pub is_reusable: bool,
    pub parameters: HashMap<String, String>,
}

/// Resolve a human-readable Bitcoin address
pub async fn resolve(&self, user: &str, domain: &str) -> Result<PaymentInstruction> {
    // Construct DNS name
    let dns_name = format!("{}.user._bitcoin-payment.{}", user, domain);

    // Query TXT records - with opts.validate=true, this will fail if DNSSEC validation fails
    let response = self.resolver.txt_lookup(&dns_name).await?;

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
