//! Lightning Address Implementation
//!
//! This module provides functionality for resolving Lightning addresses
//! to obtain Lightning invoices. Lightning addresses are user-friendly
//! identifiers that look like email addresses (e.g., user@domain.com).
//!
//! Lightning addresses are converted to LNURL-pay endpoints following the spec:
//! <https://domain.com/.well-known/lnurlp/user>

use std::str::FromStr;
use std::sync::Arc;

use lightning_invoice::Bolt11Invoice;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::instrument;
use url::Url;

use crate::wallet::MintConnector;
use crate::Amount;

/// Lightning Address Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Lightning address format
    #[error("Invalid Lightning address format: {0}")]
    InvalidFormat(String),
    /// Invalid URL
    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    /// Failed to fetch pay request data
    #[error("Failed to fetch pay request data: {0}")]
    FetchPayRequest(#[from] crate::Error),
    /// Lightning address service error
    #[error("Lightning address service error: {0}")]
    Service(String),
    /// Amount below minimum
    #[error("Amount {amount} msat is below minimum {min} msat")]
    AmountBelowMinimum { amount: u64, min: u64 },
    /// Amount above maximum
    #[error("Amount {amount} msat is above maximum {max} msat")]
    AmountAboveMaximum { amount: u64, max: u64 },
    /// No invoice in response
    #[error("No invoice in response")]
    NoInvoice,
    /// Failed to parse invoice
    #[error("Failed to parse invoice: {0}")]
    InvoiceParse(String),
}

/// Lightning address - represents a user@domain.com address
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LightningAddress {
    /// The user part of the address (before @)
    user: String,
    /// The domain part of the address (after @)
    domain: String,
}

impl LightningAddress {
    /// Convert the Lightning address to an HTTPS URL for the LNURL-pay endpoint
    fn to_url(&self) -> Result<Url, Error> {
        // Lightning address spec: https://domain.com/.well-known/lnurlp/user
        let url_str = format!("https://{}/.well-known/lnurlp/{}", self.domain, self.user);
        Ok(Url::parse(&url_str)?)
    }

    /// Fetch the LNURL-pay metadata from the service
    #[instrument(skip(client))]
    async fn fetch_pay_request_data(
        &self,
        client: &Arc<dyn MintConnector + Send + Sync>,
    ) -> Result<LnurlPayResponse, Error> {
        let url = self.to_url()?;

        tracing::debug!("Fetching Lightning address pay data from: {}", url);

        // Make HTTP GET request to fetch the pay request data
        let lnurl_response = client.fetch_lnurl_pay_request(url.as_str()).await?;

        // Validate the response
        if let Some(ref reason) = lnurl_response.reason {
            return Err(Error::Service(reason.clone()));
        }

        Ok(lnurl_response)
    }

    /// Request an invoice from the Lightning address service with a specific amount
    #[instrument(skip(client))]
    pub(crate) async fn request_invoice(
        &self,
        client: &Arc<dyn MintConnector + Send + Sync>,
        amount_msat: Amount,
    ) -> Result<Bolt11Invoice, Error> {
        let pay_data = self.fetch_pay_request_data(client).await?;

        // Validate amount is within acceptable range
        let amount_msat_u64: u64 = amount_msat.into();
        if amount_msat_u64 < pay_data.min_sendable {
            return Err(Error::AmountBelowMinimum {
                amount: amount_msat_u64,
                min: pay_data.min_sendable,
            });
        }
        if amount_msat_u64 > pay_data.max_sendable {
            return Err(Error::AmountAboveMaximum {
                amount: amount_msat_u64,
                max: pay_data.max_sendable,
            });
        }

        // Build callback URL with amount parameter
        let mut callback_url = Url::parse(&pay_data.callback)?;

        callback_url
            .query_pairs_mut()
            .append_pair("amount", &amount_msat_u64.to_string());

        tracing::debug!("Requesting invoice from callback: {}", callback_url);

        // Fetch the invoice
        let invoice_response = client.fetch_lnurl_invoice(callback_url.as_str()).await?;

        // Check for errors
        if let Some(ref reason) = invoice_response.reason {
            return Err(Error::Service(reason.clone()));
        }

        // Parse and return the invoice
        let pr = invoice_response.pr.ok_or(Error::NoInvoice)?;

        Bolt11Invoice::from_str(&pr).map_err(|e| Error::InvoiceParse(e.to_string()))
    }
}

impl FromStr for LightningAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();

        // Parse Lightning address (user@domain)
        if !trimmed.contains('@') {
            return Err(Error::InvalidFormat("must contain '@'".to_string()));
        }

        let parts: Vec<&str> = trimmed.split('@').collect();
        if parts.len() != 2 {
            return Err(Error::InvalidFormat("must be user@domain".to_string()));
        }

        let user = parts[0].trim();
        let domain = parts[1].trim();

        if user.is_empty() || domain.is_empty() {
            return Err(Error::InvalidFormat(
                "user and domain must not be empty".to_string(),
            ));
        }

        Ok(LightningAddress {
            user: user.to_string(),
            domain: domain.to_string(),
        })
    }
}

impl std::fmt::Display for LightningAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.user, self.domain)
    }
}

/// LNURL-pay response from the initial request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LnurlPayResponse {
    /// Callback URL for requesting invoice
    pub callback: String,
    /// Minimum amount in millisatoshis
    #[serde(rename = "minSendable")]
    pub min_sendable: u64,
    /// Maximum amount in millisatoshis
    #[serde(rename = "maxSendable")]
    pub max_sendable: u64,
    /// Metadata string (JSON stringified)
    pub metadata: String,
    /// Short description tag (should be "payRequest")
    pub tag: Option<String>,
    /// Optional error reason
    pub reason: Option<String>,
}

/// LNURL-pay invoice response from the callback
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LnurlPayInvoiceResponse {
    /// The BOLT11 payment request (invoice)
    pub pr: Option<String>,
    /// Optional success action
    pub success_action: Option<serde_json::Value>,
    /// Optional routes (deprecated)
    pub routes: Option<Vec<serde_json::Value>>,
    /// Optional error reason
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lightning_address_parsing() {
        let addr = LightningAddress::from_str("satoshi@bitcoin.org").unwrap();
        assert_eq!(addr.user, "satoshi");
        assert_eq!(addr.domain, "bitcoin.org");
        assert_eq!(addr.to_string(), "satoshi@bitcoin.org");
    }

    #[test]
    fn test_lightning_address_to_url() {
        let addr = LightningAddress {
            user: "alice".to_string(),
            domain: "example.com".to_string(),
        };

        let url = addr.to_url().unwrap();
        assert_eq!(url.as_str(), "https://example.com/.well-known/lnurlp/alice");
    }

    #[test]
    fn test_invalid_lightning_address() {
        assert!(LightningAddress::from_str("invalid").is_err());
        assert!(LightningAddress::from_str("@example.com").is_err());
        assert!(LightningAddress::from_str("user@").is_err());
        assert!(LightningAddress::from_str("user").is_err());
    }
}
