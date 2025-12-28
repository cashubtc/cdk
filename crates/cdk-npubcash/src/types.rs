//! Type definitions for NpubCash API

use std::str::FromStr;

use cashu::nut00::KnownMethod;
use cashu::PaymentMethod;
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{CurrencyUnit, MintQuoteState};
use cdk_common::wallet::MintQuote;
use cdk_common::Amount;
use serde::{Deserialize, Serialize};

/// Default mint URL used when quote doesn't specify one
const DEFAULT_MINT_URL: &str = "http://localhost:3338";

/// A quote from the NpubCash service
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    /// Unique identifier for the quote
    #[serde(rename = "quoteId")]
    pub id: String,
    /// Amount in the specified unit
    pub amount: u64,
    /// Currency or unit for the amount (optional, defaults to "sat")
    #[serde(default = "default_unit")]
    pub unit: String,
    /// Unix timestamp when the quote was created
    #[serde(default)]
    pub created_at: u64,
    /// Unix timestamp when the quote was paid (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_at: Option<u64>,
    /// Unix timestamp when the quote expires (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Mint URL associated with the quote (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mint_url: Option<String>,
    /// Lightning invoice request (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// Quote state (e.g., "PAID", "PENDING") (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Whether the quote is locked (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
}

fn default_unit() -> String {
    "sat".to_string()
}

/// Response containing a list of quotes with pagination metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotesResponse {
    /// Quote data
    pub data: QuotesData,
    /// Pagination metadata
    pub metadata: Metadata,
}

/// Container for quote data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotesData {
    /// List of quotes
    pub quotes: Vec<Quote>,
}

/// Pagination metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Total number of available items
    pub total: usize,
    /// Current offset (optional, may not be present in all responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Items per page
    pub limit: usize,
    /// Since timestamp (optional, present when querying with since parameter)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

/// Response containing user settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    /// Whether the request resulted in an error
    #[serde(default)]
    pub error: bool,
    /// User data container
    pub data: UserDataContainer,
}

/// Container for user data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDataContainer {
    /// User settings
    pub user: UserData,
}

/// User settings data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserData {
    /// User's public key
    pub pubkey: String,
    /// Configured mint URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mint_url: Option<String>,
    /// Whether quotes are locked
    #[serde(default)]
    pub lock_quote: bool,
}

/// NIP-98 authentication response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nip98Response {
    /// NIP-98 response data
    pub data: Nip98Data,
}

/// NIP-98 token data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nip98Data {
    /// JWT token
    pub token: String,
}

impl From<Quote> for MintQuote {
    fn from(quote: Quote) -> Self {
        let mint_url = quote
            .mint_url
            .and_then(|url| MintUrl::from_str(&url).ok())
            .unwrap_or_else(|| {
                MintUrl::from_str(DEFAULT_MINT_URL).expect("default mint URL should be valid")
            });

        let unit = CurrencyUnit::from_str(&quote.unit).unwrap_or(CurrencyUnit::Sat);

        let state = match quote.state.as_deref() {
            Some("PAID") => MintQuoteState::Paid,
            Some("ISSUED") => MintQuoteState::Issued,
            _ => MintQuoteState::Unpaid,
        };

        let expiry = quote.expires_at.unwrap_or(quote.created_at + 86400);

        Self {
            id: quote.id,
            mint_url,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount: Some(Amount::from(quote.amount)),
            unit,
            request: quote.request.unwrap_or_default(),
            state,
            expiry,
            secret_key: None,
            amount_issued: Amount::ZERO,
            amount_paid: if quote.paid_at.is_some() {
                Amount::from(quote.amount)
            } else {
                Amount::ZERO
            },
            // REVIEW: Where does this get set when using npubcash with saga
            used_by_operation: None,
        }
    }
}
