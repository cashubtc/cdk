//! Quote-related FFI types

use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::mint::MintUrl;
use crate::error::FfiError;

/// FFI-compatible MintQuote
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintQuote {
    /// Quote ID
    pub id: String,
    /// Quote amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Payment request
    pub request: String,
    /// Quote state
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Amount issued
    pub amount_issued: Amount,
    /// Amount paid
    pub amount_paid: Amount,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Secret key (optional, hex-encoded)
    pub secret_key: Option<String>,
    /// Operation ID that reserved this quote
    pub used_by_operation: Option<String>,
    /// Version for optimistic locking
    #[serde(default)]
    pub version: u32,
}

impl From<cdk::wallet::MintQuote> for MintQuote {
    fn from(quote: cdk::wallet::MintQuote) -> Self {
        Self {
            id: quote.id.clone(),
            amount: quote.amount.map(Into::into),
            unit: quote.unit.clone().into(),
            request: quote.request.clone(),
            state: quote.state.into(),
            expiry: quote.expiry,
            mint_url: quote.mint_url.clone().into(),
            amount_issued: quote.amount_issued.into(),
            amount_paid: quote.amount_paid.into(),
            payment_method: quote.payment_method.into(),
            secret_key: quote.secret_key.map(|sk| sk.to_secret_hex()),
            used_by_operation: quote.used_by_operation.map(|id| id.to_string()),
            version: quote.version,
        }
    }
}

impl TryFrom<MintQuote> for cdk::wallet::MintQuote {
    type Error = FfiError;

    fn try_from(quote: MintQuote) -> Result<Self, Self::Error> {
        let secret_key = quote
            .secret_key
            .map(|hex| cdk::nuts::SecretKey::from_hex(&hex))
            .transpose()
            .map_err(|e| FfiError::internal(format!("Invalid secret key: {}", e)))?;

        Ok(Self {
            id: quote.id,
            amount: quote.amount.map(Into::into),
            unit: quote.unit.into(),
            request: quote.request,
            state: quote.state.into(),
            expiry: quote.expiry,
            mint_url: quote.mint_url.try_into()?,
            amount_issued: quote.amount_issued.into(),
            amount_paid: quote.amount_paid.into(),
            payment_method: quote.payment_method.into(),
            secret_key,
            used_by_operation: quote.used_by_operation,
            version: quote.version,
        })
    }
}

/// Get total amount for a mint quote (amount paid)
#[uniffi::export]
pub fn mint_quote_total_amount(quote: &MintQuote) -> Result<Amount, FfiError> {
    let cdk_quote: cdk::wallet::MintQuote = quote.clone().try_into()?;
    Ok(cdk_quote.total_amount().into())
}

/// Check if mint quote is expired
#[uniffi::export]
pub fn mint_quote_is_expired(quote: &MintQuote, current_time: u64) -> Result<bool, FfiError> {
    let cdk_quote: cdk::wallet::MintQuote = quote.clone().try_into()?;
    Ok(cdk_quote.is_expired(current_time))
}

/// Get amount that can be minted from a mint quote
#[uniffi::export]
pub fn mint_quote_amount_mintable(quote: &MintQuote) -> Result<Amount, FfiError> {
    let cdk_quote: cdk::wallet::MintQuote = quote.clone().try_into()?;
    Ok(cdk_quote.amount_mintable().into())
}

/// Decode MintQuote from JSON string
#[uniffi::export]
pub fn decode_mint_quote(json: String) -> Result<MintQuote, FfiError> {
    let quote: cdk::wallet::MintQuote = serde_json::from_str(&json)?;
    Ok(quote.into())
}

/// Encode MintQuote to JSON string
#[uniffi::export]
pub fn encode_mint_quote(quote: MintQuote) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&quote)?)
}

/// FFI-compatible MintQuoteBolt11Response
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintQuoteBolt11Response {
    /// Quote ID
    pub quote: String,
    /// Request string
    pub request: String,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp (optional)
    pub expiry: Option<u64>,
    /// Amount (optional)
    pub amount: Option<Amount>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
    /// Pubkey (optional)
    pub pubkey: Option<String>,
}

impl From<cdk::nuts::MintQuoteBolt11Response<String>> for MintQuoteBolt11Response {
    fn from(response: cdk::nuts::MintQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            request: response.request,
            state: response.state.into(),
            expiry: response.expiry,
            amount: response.amount.map(Into::into),
            unit: response.unit.map(Into::into),
            pubkey: response.pubkey.map(|p| p.to_string()),
        }
    }
}

/// FFI-compatible MintQuoteCustomResponse
///
/// This is a unified response type for custom payment methods that includes
/// extra fields for method-specific data (e.g., ehash share).
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintQuoteCustomResponse {
    /// Quote ID
    pub quote: String,
    /// Request string
    pub request: String,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp (optional)
    pub expiry: Option<u64>,
    /// Amount (optional)
    pub amount: Option<Amount>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
    /// Pubkey (optional)
    pub pubkey: Option<String>,
    /// Extra payment-method-specific fields as JSON string
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    pub extra: Option<String>,
}

impl From<cdk::nuts::MintQuoteCustomResponse<String>> for MintQuoteCustomResponse {
    fn from(response: cdk::nuts::MintQuoteCustomResponse<String>) -> Self {
        let extra = if response.extra.is_null() {
            None
        } else {
            Some(response.extra.to_string())
        };

        Self {
            quote: response.quote,
            request: response.request,
            state: response.state.into(),
            expiry: response.expiry,
            amount: response.amount.map(Into::into),
            unit: response.unit.map(Into::into),
            pubkey: response.pubkey.map(|p| p.to_string()),
            extra,
        }
    }
}

/// FFI-compatible MeltQuoteBolt11Response
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuoteBolt11Response {
    /// Quote ID
    pub quote: String,
    /// Amount
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment preimage (optional)
    pub payment_preimage: Option<String>,
    /// Request string (optional)
    pub request: Option<String>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
}

impl From<cdk::nuts::MeltQuoteBolt11Response<String>> for MeltQuoteBolt11Response {
    fn from(response: cdk::nuts::MeltQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            amount: response.amount.into(),
            fee_reserve: response.fee_reserve.into(),
            state: response.state.into(),
            expiry: response.expiry,
            payment_preimage: response.payment_preimage,
            request: response.request,
            unit: response.unit.map(Into::into),
        }
    }
}

/// FFI-compatible MeltQuoteCustomResponse
///
/// This is a unified response type for custom payment methods that includes
/// extra fields for method-specific data.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuoteCustomResponse {
    /// Quote ID
    pub quote: String,
    /// Amount
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment preimage (optional)
    pub payment_preimage: Option<String>,
    /// Request string (optional)
    pub request: Option<String>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
    /// Extra payment-method-specific fields as JSON string
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    pub extra: Option<String>,
}

impl From<cdk::nuts::MeltQuoteCustomResponse<String>> for MeltQuoteCustomResponse {
    fn from(response: cdk::nuts::MeltQuoteCustomResponse<String>) -> Self {
        let extra = if response.extra.is_null() {
            None
        } else {
            Some(response.extra.to_string())
        };

        Self {
            quote: response.quote,
            amount: response.amount.into(),
            fee_reserve: response.fee_reserve.into(),
            state: response.state.into(),
            expiry: response.expiry,
            payment_preimage: response.payment_preimage,
            request: response.request,
            unit: response.unit.map(Into::into),
            extra,
        }
    }
}

/// FFI-compatible PaymentMethod
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum PaymentMethod {
    /// Bolt11 payment type
    Bolt11,
    /// Bolt12 payment type
    Bolt12,
    /// Custom payment type
    Custom { method: String },
}

impl From<cdk::nuts::PaymentMethod> for PaymentMethod {
    fn from(method: cdk::nuts::PaymentMethod) -> Self {
        match method.as_str() {
            "bolt11" => Self::Bolt11,
            "bolt12" => Self::Bolt12,
            s => Self::Custom {
                method: s.to_string(),
            },
        }
    }
}

impl From<PaymentMethod> for cdk::nuts::PaymentMethod {
    fn from(method: PaymentMethod) -> Self {
        match method {
            PaymentMethod::Bolt11 => Self::from("bolt11"),
            PaymentMethod::Bolt12 => Self::from("bolt12"),
            PaymentMethod::Custom { method } => Self::from(method),
        }
    }
}

/// FFI-compatible MeltQuote
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuote {
    /// Quote ID
    pub id: String,
    /// Quote amount
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Payment request
    pub request: String,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Operation ID that reserved this quote
    pub used_by_operation: Option<String>,
    /// Version for optimistic locking
    #[serde(default)]
    pub version: u32,
}

impl From<cdk::wallet::MeltQuote> for MeltQuote {
    fn from(quote: cdk::wallet::MeltQuote) -> Self {
        Self {
            id: quote.id.clone(),
            amount: quote.amount.into(),
            unit: quote.unit.clone().into(),
            request: quote.request.clone(),
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_preimage: quote.payment_preimage.clone(),
            payment_method: quote.payment_method.into(),
            used_by_operation: quote.used_by_operation.map(|id| id.to_string()),
            version: quote.version,
        }
    }
}

impl TryFrom<MeltQuote> for cdk::wallet::MeltQuote {
    type Error = FfiError;

    fn try_from(quote: MeltQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            id: quote.id,
            amount: quote.amount.into(),
            unit: quote.unit.into(),
            request: quote.request,
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_preimage: quote.payment_preimage,
            payment_method: quote.payment_method.into(),
            used_by_operation: quote.used_by_operation,
            version: quote.version,
        })
    }
}

impl MeltQuote {
    /// Convert MeltQuote to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode MeltQuote from JSON string
#[uniffi::export]
pub fn decode_melt_quote(json: String) -> Result<MeltQuote, FfiError> {
    let quote: cdk::wallet::MeltQuote = serde_json::from_str(&json)?;
    Ok(quote.into())
}

/// Encode MeltQuote to JSON string
#[uniffi::export]
pub fn encode_melt_quote(quote: MeltQuote) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&quote)?)
}

/// FFI-compatible QuoteState
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum QuoteState {
    Unpaid,
    Paid,
    Pending,
    Issued,
}

impl From<cdk::nuts::nut05::QuoteState> for QuoteState {
    fn from(state: cdk::nuts::nut05::QuoteState) -> Self {
        match state {
            cdk::nuts::nut05::QuoteState::Unpaid => QuoteState::Unpaid,
            cdk::nuts::nut05::QuoteState::Paid => QuoteState::Paid,
            cdk::nuts::nut05::QuoteState::Pending => QuoteState::Pending,
            cdk::nuts::nut05::QuoteState::Unknown => QuoteState::Unpaid,
            cdk::nuts::nut05::QuoteState::Failed => QuoteState::Unpaid,
        }
    }
}

impl From<QuoteState> for cdk::nuts::nut05::QuoteState {
    fn from(state: QuoteState) -> Self {
        match state {
            QuoteState::Unpaid => cdk::nuts::nut05::QuoteState::Unpaid,
            QuoteState::Paid => cdk::nuts::nut05::QuoteState::Paid,
            QuoteState::Pending => cdk::nuts::nut05::QuoteState::Pending,
            QuoteState::Issued => cdk::nuts::nut05::QuoteState::Paid, // Map issued to paid for melt quotes
        }
    }
}

impl From<cdk::nuts::MintQuoteState> for QuoteState {
    fn from(state: cdk::nuts::MintQuoteState) -> Self {
        match state {
            cdk::nuts::MintQuoteState::Unpaid => QuoteState::Unpaid,
            cdk::nuts::MintQuoteState::Paid => QuoteState::Paid,
            cdk::nuts::MintQuoteState::Issued => QuoteState::Issued,
        }
    }
}

impl From<QuoteState> for cdk::nuts::MintQuoteState {
    fn from(state: QuoteState) -> Self {
        match state {
            QuoteState::Unpaid => cdk::nuts::MintQuoteState::Unpaid,
            QuoteState::Paid => cdk::nuts::MintQuoteState::Paid,
            QuoteState::Issued => cdk::nuts::MintQuoteState::Issued,
            QuoteState::Pending => cdk::nuts::MintQuoteState::Paid, // Map pending to paid
        }
    }
}

// Note: MeltQuoteState is the same as nut05::QuoteState, so we don't need a separate impl
