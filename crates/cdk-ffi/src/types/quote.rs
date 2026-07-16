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
    /// Requested or fixed quote amount, when defined by the payment method.
    ///
    /// Variable-amount methods such as onchain leave this unset and track
    /// funds through `amount_paid` and `amount_issued`.
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
    /// Last update timestamp
    #[serde(default)]
    pub updated_at: u64,
    /// Estimated confirmation target in blocks for onchain quotes
    pub estimated_blocks: Option<u32>,
    /// Optional onchain Payjoin instructions returned by the mint.
    pub payjoin: Option<PayjoinV2>,
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
            updated_at: quote.updated_at,
            estimated_blocks: quote.estimated_blocks,
            payjoin: quote.payjoin.map(Into::into),
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
            updated_at: quote.updated_at,
            estimated_blocks: quote.estimated_blocks,
            payjoin: quote.payjoin.map(TryInto::try_into).transpose()?,
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
    /// Payment method
    pub method: PaymentMethod,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp (optional)
    pub expiry: Option<u64>,
    /// Amount (optional)
    pub amount: Option<Amount>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
    /// Amount paid
    pub amount_paid: Amount,
    /// Amount issued
    pub amount_issued: Amount,
    /// Last update timestamp
    pub updated_at: u64,
    /// Pubkey (optional)
    pub pubkey: Option<String>,
}

impl From<cdk::nuts::MintQuoteBolt11Response<String>> for MintQuoteBolt11Response {
    fn from(response: cdk::nuts::MintQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            request: response.request,
            method: response.method.into(),
            state: response.state.into(),
            expiry: response.expiry,
            amount: response.amount.map(Into::into),
            unit: response.unit.map(Into::into),
            amount_paid: response.amount_paid.into(),
            amount_issued: response.amount_issued.into(),
            updated_at: response.updated_at,
            pubkey: response.pubkey.map(|p| p.to_string()),
        }
    }
}

impl From<cdk::wallet::MintQuote> for MintQuoteBolt11Response {
    fn from(quote: cdk::wallet::MintQuote) -> Self {
        Self {
            quote: quote.id,
            request: quote.request,
            method: quote.payment_method.into(),
            state: quote.state.into(),
            expiry: Some(quote.expiry),
            amount: quote.amount.map(Into::into),
            unit: Some(quote.unit.into()),
            amount_paid: quote.amount_paid.into(),
            amount_issued: quote.amount_issued.into(),
            updated_at: quote.updated_at,
            pubkey: quote.secret_key.map(|sk| sk.public_key().to_string()),
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
    /// Payment method
    pub method: PaymentMethod,
    /// Expiry timestamp (optional)
    pub expiry: Option<u64>,
    /// Amount (optional)
    pub amount: Option<Amount>,
    /// Amount paid
    pub amount_paid: Amount,
    /// Amount issued
    pub amount_issued: Amount,
    /// Last update timestamp
    pub updated_at: u64,
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
            method: response.method.into(),
            expiry: response.expiry,
            amount: response.amount.map(Into::into),
            amount_paid: response.amount_paid.into(),
            amount_issued: response.amount_issued.into(),
            updated_at: response.updated_at,
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
    /// Payment method
    pub method: PaymentMethod,
    /// Amount
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment proof (optional)
    pub payment_proof: Option<String>,
    /// Request string (optional)
    pub request: Option<String>,
    /// Unit (optional)
    pub unit: Option<CurrencyUnit>,
}

impl From<cdk::nuts::MeltQuoteBolt11Response<String>> for MeltQuoteBolt11Response {
    fn from(response: cdk::nuts::MeltQuoteBolt11Response<String>) -> Self {
        Self {
            quote: response.quote,
            method: response.method.into(),
            amount: response.amount.into(),
            fee_reserve: response.fee_reserve.into(),
            state: response.state.into(),
            expiry: response.expiry,
            payment_proof: response.payment_preimage,
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
    /// Payment method
    pub method: PaymentMethod,
    /// Amount
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Option<Amount>,
    /// State of the quote
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Payment proof (optional)
    pub payment_proof: Option<String>,
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
            method: response.method.into(),
            amount: response.amount.into(),
            fee_reserve: response.fee_reserve.map(Into::into),
            state: response.state.into(),
            expiry: response.expiry,
            payment_proof: response.payment_preimage,
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
    /// Onchain Bitcoin payment type
    Onchain,
    /// Custom payment type
    Custom { method: String },
}

impl From<cdk::nuts::PaymentMethod> for PaymentMethod {
    fn from(method: cdk::nuts::PaymentMethod) -> Self {
        match method.as_str() {
            "bolt11" => Self::Bolt11,
            "bolt12" => Self::Bolt12,
            "onchain" => Self::Onchain,
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
            PaymentMethod::Onchain => Self::from("onchain"),
            PaymentMethod::Custom { method } => Self::from(method),
        }
    }
}

/// FFI-compatible Payjoin v2 parameters.
///
/// Cashu uses Unix timestamp; BIP77 URI fragments use encoded `EX1`.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct PayjoinV2 {
    /// Mailbox endpoint without BIP77 fragment parameters.
    pub endpoint: String,
    /// Encoded OHTTP key material.
    pub ohttp_keys: String,
    /// Encoded receiver session key.
    pub receiver_key: String,
    /// Unix timestamp until the Payjoin parameters are valid.
    pub expires_at: u64,
}

impl From<cdk::nuts::PayjoinV2> for PayjoinV2 {
    fn from(payjoin: cdk::nuts::PayjoinV2) -> Self {
        Self {
            endpoint: payjoin.endpoint,
            ohttp_keys: payjoin.ohttp_keys.to_string(),
            receiver_key: payjoin.receiver_key.to_string(),
            expires_at: payjoin.expires_at,
        }
    }
}

impl TryFrom<PayjoinV2> for cdk::nuts::PayjoinV2 {
    type Error = cdk::nuts::nut31::PayjoinV2KeyError;

    fn try_from(payjoin: PayjoinV2) -> Result<Self, Self::Error> {
        Self::new(
            payjoin.endpoint,
            payjoin.ohttp_keys,
            payjoin.receiver_key,
            payjoin.expires_at,
        )
    }
}

/// FFI-compatible MintQuoteOnchainResponse.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MintQuoteOnchainResponse {
    /// Quote ID
    pub quote: String,
    /// Bitcoin address to pay
    pub request: String,
    /// Payment method
    pub method: PaymentMethod,
    /// Unit
    pub unit: CurrencyUnit,
    /// Expiry timestamp
    pub expiry: Option<u64>,
    /// NUT-20 public key
    pub pubkey: String,
    /// Total confirmed amount paid to the onchain address
    pub amount_paid: Amount,
    /// Amount already issued for this quote
    pub amount_issued: Amount,
    /// Last update timestamp
    pub updated_at: u64,
    /// Optional Payjoin instructions.
    pub payjoin: Option<PayjoinV2>,
}

impl From<cdk::nuts::MintQuoteOnchainResponse<String>> for MintQuoteOnchainResponse {
    fn from(response: cdk::nuts::MintQuoteOnchainResponse<String>) -> Self {
        Self {
            quote: response.quote,
            request: response.request,
            method: response.method.into(),
            unit: response.unit.into(),
            expiry: response.expiry,
            pubkey: response.pubkey.to_string(),
            amount_paid: response.amount_paid.into(),
            amount_issued: response.amount_issued.into(),
            updated_at: response.updated_at,
            payjoin: response.payjoin.map(Into::into),
        }
    }
}

/// Fee option for an onchain melt quote.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuoteOnchainFeeOption {
    /// Server-assigned identifier the wallet echoes back to select this option
    pub fee_index: u32,
    /// Maximum onchain transaction fee the mint may charge
    pub fee_reserve: Amount,
    /// Estimated confirmation target in blocks
    pub estimated_blocks: u32,
}

impl From<cdk::nuts::nut30::MeltQuoteOnchainFeeOption> for MeltQuoteOnchainFeeOption {
    fn from(option: cdk::nuts::nut30::MeltQuoteOnchainFeeOption) -> Self {
        Self {
            fee_index: option.fee_index,
            fee_reserve: option.fee_reserve.into(),
            estimated_blocks: option.estimated_blocks,
        }
    }
}

/// FFI-compatible MeltQuoteOnchainResponse.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuoteOnchainResponse {
    /// Quote ID
    pub quote: String,
    /// Payment method
    pub method: PaymentMethod,
    /// Amount being paid to the onchain address
    pub amount: Amount,
    /// Unit
    pub unit: CurrencyUnit,
    /// Quote state
    pub state: QuoteState,
    /// Expiry timestamp
    pub expiry: u64,
    /// Bitcoin address to pay
    pub request: String,
    /// Available onchain fee options
    pub fee_options: Vec<MeltQuoteOnchainFeeOption>,
    /// Selected fee option index, once execution has started
    pub selected_fee_index: Option<u32>,
    /// Broadcast outpoint (`txid:vout`), once available
    pub outpoint: Option<String>,
    /// Change blind signatures as JSON, when the mint returns change
    pub change: Option<String>,
    /// Optional Payjoin v2 acceptance for this quote.
    pub payjoin: Option<PayjoinV2>,
}

impl From<cdk::nuts::MeltQuoteOnchainResponse<String>> for MeltQuoteOnchainResponse {
    fn from(response: cdk::nuts::MeltQuoteOnchainResponse<String>) -> Self {
        let change = response
            .change
            .as_ref()
            .and_then(|change| serde_json::to_string(change).ok());

        Self {
            quote: response.quote,
            method: response.method.into(),
            amount: response.amount.into(),
            unit: response.unit.into(),
            state: response.state.into(),
            expiry: response.expiry,
            request: response.request,
            fee_options: response.fee_options.into_iter().map(Into::into).collect(),
            selected_fee_index: response.selected_fee_index,
            outpoint: response.outpoint,
            change,
            payjoin: response.payjoin.map(Into::into),
        }
    }
}

/// FFI-compatible MeltQuote
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct MeltQuote {
    /// Quote ID
    pub id: String,
    /// Mint URL
    pub mint_url: Option<MintUrl>,
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
    /// Payment proof (e.g. Lightning preimage or onchain outpoint)
    pub payment_proof: Option<String>,
    /// Estimated confirmation target in blocks for onchain quotes
    pub estimated_blocks: Option<u32>,
    /// Selected fee option index for onchain quotes
    pub fee_index: Option<u32>,
    /// Optional onchain Payjoin acceptance returned by the mint.
    pub payjoin: Option<PayjoinV2>,
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
            mint_url: quote.mint_url.map(Into::into),
            amount: quote.amount.into(),
            unit: quote.unit.clone().into(),
            request: quote.request.clone(),
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_proof: quote.payment_proof.clone(),
            estimated_blocks: quote.estimated_blocks,
            fee_index: quote.fee_index,
            payjoin: quote.payjoin.map(Into::into),
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
            mint_url: quote.mint_url.map(|m| m.try_into()).transpose()?,
            amount: quote.amount.into(),
            unit: quote.unit.into(),
            request: quote.request,
            fee_reserve: quote.fee_reserve.into(),
            state: quote.state.into(),
            expiry: quote.expiry,
            payment_proof: quote.payment_proof,
            estimated_blocks: quote.estimated_blocks,
            fee_index: quote.fee_index,
            payjoin: quote.payjoin.map(TryInto::try_into).transpose()?,
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
            QuoteState::Pending => cdk::nuts::MintQuoteState::Unpaid,
        }
    }
}

// Note: MeltQuoteState is the same as nut05::QuoteState, so we don't need a separate impl
