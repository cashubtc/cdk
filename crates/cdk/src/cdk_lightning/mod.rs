//! CDK Mint Lightning

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use lightning::offers::offer::Offer;
use lightning_invoice::{Bolt11Invoice, ParseOrSemanticError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::nut20::MeltQuoteBolt12Request;
use crate::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use crate::{mint, Amount};

/// CDK Lightning Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invoice already paid
    #[error("Invoice already paid")]
    InvoiceAlreadyPaid,
    /// Invoice pay pending
    #[error("Invoice pay is pending")]
    InvoicePaymentPending,
    /// Invoice amount unknown
    #[error("Invoice amount unknown")]
    InvoiceAmountUnknown,
    /// Unsupported unit
    #[error("Unsupported unit")]
    UnsupportedUnit,
    /// Payment state is unknown
    #[error("Payment state is unknown")]
    UnknownPaymentState,
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
    /// Lightning Error
    #[error(transparent)]
    Lightning(Box<dyn std::error::Error + Send + Sync>),
    /// Serde Error
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    /// AnyHow Error
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
    /// Parse Error
    #[error(transparent)]
    Parse(#[from] ParseOrSemanticError),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] crate::amount::Error),
}

/// MintLighting Trait
#[async_trait]
pub trait MintLightning {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Base Unit
    fn get_settings(&self) -> Settings;

    /// Create a new invoice
    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err>;

    /// Get payment quote
    /// Used to get fee and amount required for a payment request
    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err>;

    /// Pay bolt11 invoice
    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err>;

    /// Listen for invoices to be paid to the mint
    /// Returns a stream of request_lookup_id once invoices are paid
    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = (String, Amount)> + Send>>, Self::Err>;

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool;

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self);

    /// Check the status of an incoming payment
    async fn check_incoming_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err>;

    /// Check the status of an outgoing payment
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &str,
    ) -> Result<PayInvoiceResponse, Self::Err>;

    /// Bolt12 Payment quote
    async fn get_bolt12_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err>;

    /// Pay a bolt12 offer
    async fn pay_bolt12_offer(
        &self,
        melt_quote: mint::MeltQuote,
        amount: Option<Amount>,
        max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err>;

    /// Create bolt12 offer
    async fn create_bolt12_offer(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
        single_use: bool,
    ) -> Result<CreateOfferResponse, Self::Err>;
}

/// Create invoice response
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CreateInvoiceResponse {
    /// Id that is used to look up the invoice from the ln backend
    pub request_lookup_id: String,
    /// Bolt11 payment request
    pub request: Bolt11Invoice,
    /// Unix Expiry of Invoice
    pub expiry: Option<u64>,
}

/// Create offer response
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CreateOfferResponse {
    /// Id that is used to look up the invoice from the ln backend
    pub request_lookup_id: String,
    /// Bolt11 payment request
    pub request: Offer,
    /// Unix Expiry of Invoice
    pub expiry: Option<u64>,
}

/// Pay invoice response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayInvoiceResponse {
    /// Payment hash
    pub payment_lookup_id: String,
    /// Payment Preimage
    pub payment_preimage: Option<String>,
    /// Status
    pub status: MeltQuoteState,
    /// Total Amount Spent
    pub total_spent: Amount,
    /// Unit of total spent
    pub unit: CurrencyUnit,
}

/// Payment quote response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentQuoteResponse {
    /// Request look up id
    pub request_lookup_id: String,
    /// Amount
    pub amount: Amount,
    /// Fee required for melt
    pub fee: Amount,
    /// Status
    pub state: MeltQuoteState,
}

/// Payment quote response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bolt12PaymentQuoteResponse {
    /// Request look up id
    pub request_lookup_id: String,
    /// Amount
    pub amount: Amount,
    /// Fee required for melt
    pub fee: Amount,
    /// Status
    pub state: MeltQuoteState,
    /// Bolt12 invoice
    pub invoice: Option<String>,
}

/// Ln backend settings
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// MPP supported
    pub mpp: bool,
    /// Supports bolt12 mint
    pub bolt12_mint: bool,
    /// Supports bolt12 melt
    pub bolt12_melt: bool,
    /// Base unit of backend
    pub unit: CurrencyUnit,
    /// Invoice Description supported
    pub invoice_description: bool,
}
