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
    /// Returned invoice does not contain an amount
    #[error("Returned invoice does not contain an amount")]
    InvoiceAmountUndefined,
    /// Returned invoice amount does not match the requested amount
    #[error(
        "Returned invoice amount {actual} msat does not match requested amount {expected} msat"
    )]
    IncorrectInvoiceAmount { actual: u64, expected: u64 },
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
        let mut url = Url::parse(&format!("https://{}/", self.domain))?;
        url.path_segments_mut()
            .map_err(|_| Error::InvalidFormat("domain must be a bare host".to_string()))?
            .extend([".well-known", "lnurlp", &self.user]);
        Ok(url)
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

        let invoice =
            Bolt11Invoice::from_str(&pr).map_err(|e| Error::InvoiceParse(e.to_string()))?;
        let invoice_amount_msat = invoice
            .amount_milli_satoshis()
            .ok_or(Error::InvoiceAmountUndefined)?;

        if invoice_amount_msat != amount_msat_u64 {
            return Err(Error::IncorrectInvoiceAmount {
                actual: invoice_amount_msat,
                expected: amount_msat_u64,
            });
        }

        Ok(invoice)
    }
}

impl FromStr for LightningAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();

        let (user, domain) = trimmed
            .split_once('@')
            .ok_or_else(|| Error::InvalidFormat("must contain '@'".to_string()))?;

        if domain.contains('@') {
            return Err(Error::InvalidFormat("must be user@domain".to_string()));
        }

        let user = user.trim();
        let domain = domain.trim();

        if user.is_empty() || domain.is_empty() {
            return Err(Error::InvalidFormat(
                "user and domain must not be empty".to_string(),
            ));
        }

        validate_lud16_user(user)?;
        validate_lud16_domain(domain)?;

        Ok(LightningAddress {
            user: user.to_string(),
            domain: domain.to_ascii_lowercase(),
        })
    }
}

fn validate_lud16_user(user: &str) -> Result<(), Error> {
    if user == "." || user == ".." {
        return Err(Error::InvalidFormat(
            "user must not be a dot segment".to_string(),
        ));
    }

    if !user.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.' | b'+')
    }) {
        return Err(Error::InvalidFormat(
            "user must match LUD-16 character set".to_string(),
        ));
    }

    Ok(())
}

fn validate_lud16_domain(domain: &str) -> Result<(), Error> {
    if domain.contains(['/', '?', '#', '@', ':'])
        || domain.chars().any(char::is_whitespace)
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err(Error::InvalidFormat(
            "domain must be a bare host".to_string(),
        ));
    }

    let url = Url::parse(&format!("https://{domain}/"))?;

    if url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::InvalidFormat(
            "domain must be a bare host".to_string(),
        ));
    }

    match url.host() {
        Some(url::Host::Domain(host)) if host != "localhost" => validate_domain_labels(host),
        Some(url::Host::Domain(_)) | Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => Err(
            Error::InvalidFormat("domain must be a public DNS host".to_string()),
        ),
        None => Err(Error::InvalidFormat(
            "domain must be a bare host".to_string(),
        )),
    }
}

fn validate_domain_labels(domain: &str) -> Result<(), Error> {
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err(Error::InvalidFormat(
            "domain must include at least two labels".to_string(),
        ));
    }

    if labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    }) {
        Ok(())
    } else {
        Err(Error::InvalidFormat(
            "domain must be a valid DNS name".to_string(),
        ))
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
    use std::sync::Arc;

    use super::*;
    use crate::wallet::test_utils::MockMintConnector;

    const INVOICE_100_SATS: &str = "lnbc1u1p53kkd9pp5ve8pd9zr60yjyvs6tn77mndavzrl5lwd2gx5hk934f6q8jwguzgsdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5482y73fxmlvg4t66nupdaph93h7dcmfsg2ud72wajf0cpk3a96rq9qxpqysgqujexd0l89u5dutn8hxnsec0c7jrt8wz0z67rut0eah0g7p6zhycn2vff0ts5vwn2h93kx8zzqy3tzu4gfhkya2zpdmqelg0ceqnjztcqma65pr";
    #[test]
    fn test_lightning_address_parsing() {
        let addr = LightningAddress::from_str("satoshi@bitcoin.org").unwrap();
        assert_eq!(addr.user, "satoshi");
        assert_eq!(addr.domain, "bitcoin.org");
        assert_eq!(addr.to_string(), "satoshi@bitcoin.org");

        let tagged_addr = LightningAddress::from_str("satoshi+tips@bitcoin.org").unwrap();
        assert_eq!(tagged_addr.user, "satoshi+tips");
        assert_eq!(tagged_addr.domain, "bitcoin.org");
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
    fn test_lightning_address_to_url_preserves_lnurlp_path() {
        let addr = LightningAddress::from_str("alice+tips@example.com").unwrap();

        let url = addr.to_url().unwrap();

        assert_eq!(
            url.as_str(),
            "https://example.com/.well-known/lnurlp/alice+tips"
        );
        assert_eq!(url.path(), "/.well-known/lnurlp/alice+tips");
    }

    #[test]
    fn test_invalid_lightning_address() {
        assert!(LightningAddress::from_str("invalid").is_err());
        assert!(LightningAddress::from_str("@example.com").is_err());
        assert!(LightningAddress::from_str("user@").is_err());
        assert!(LightningAddress::from_str("user").is_err());
    }

    #[test]
    fn test_invalid_lightning_address_user_part() {
        assert!(LightningAddress::from_str("../admin@example.com").is_err());
        assert!(LightningAddress::from_str("./admin@example.com").is_err());
        assert!(LightningAddress::from_str(".@example.com").is_err());
        assert!(LightningAddress::from_str("..@example.com").is_err());
        assert!(LightningAddress::from_str("Alice@example.com").is_err());
        assert!(LightningAddress::from_str("alice%2fadmin@example.com").is_err());
    }

    #[test]
    fn test_invalid_lightning_address_domain_part() {
        assert!(LightningAddress::from_str("alice@example.com/path").is_err());
        assert!(LightningAddress::from_str("alice@example.com?x=y").is_err());
        assert!(LightningAddress::from_str("alice@example.com#fragment").is_err());
        assert!(LightningAddress::from_str("alice@user:pass@example.com").is_err());
        assert!(LightningAddress::from_str("alice@example.com:443").is_err());
        assert!(LightningAddress::from_str("alice@127.0.0.1").is_err());
        assert!(LightningAddress::from_str("alice@[::1]").is_err());
        assert!(LightningAddress::from_str("alice@localhost").is_err());
    }

    #[tokio::test]
    async fn test_request_invoice_accepts_matching_invoice_amount() {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_lnurl_pay_request_response(Ok(LnurlPayResponse {
            callback: "https://example.com/callback".to_string(),
            min_sendable: 1,
            max_sendable: 1_000_000,
            metadata: "[]".to_string(),
            tag: Some("payRequest".to_string()),
            reason: None,
        }));
        connector.set_lnurl_invoice_response(Ok(LnurlPayInvoiceResponse {
            pr: Some(INVOICE_100_SATS.to_string()),
            success_action: None,
            routes: None,
            reason: None,
        }));

        let address = LightningAddress::from_str("alice@example.com").expect("valid address");
        let invoice = address
            .request_invoice(
                &(connector as Arc<dyn crate::wallet::MintConnector + Send + Sync>),
                Amount::from(100_000_u64),
            )
            .await
            .expect("matching amount should succeed");

        assert_eq!(invoice.amount_milli_satoshis(), Some(100_000));
    }
}
