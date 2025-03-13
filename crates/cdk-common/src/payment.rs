//! CDK Mint Lightning

use std::any::Any;
use std::pin::Pin;

use async_trait::async_trait;
use cashu::MeltOptions;
use futures::Stream;
use lightning_invoice::ParseOrSemanticError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState};
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
    /// Unsupported unit
    #[error("Unsupported unit")]
    UnsupportedUnit,
    /// Unsupported payment option
    #[error("Unsupported payment option")]
    UnsupportedPaymentOption,
    /// Payment state is unknown
    #[error("Payment state is unknown")]
    UnknownPaymentState,
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
    /// NUT04 Error
    #[error(transparent)]
    NUT04(#[from] crate::nuts::nut04::Error),
    /// NUT05 Error
    #[error(transparent)]
    NUT05(#[from] crate::nuts::nut05::Error),
    /// Custom
    #[error("`{0}`")]
    Custom(String),
}

/// The base trait for all mint payment settings.
pub trait BaseMintSettings: Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

/// Mint payment trait
#[async_trait]
pub trait MintPayment {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Fetch Mint Settings. They may be of different types, depending on the payment processor.
    async fn get_settings(&self) -> Result<Box<dyn BaseMintSettings>, Self::Err>;

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: Option<u64>,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err>;

    /// Get payment quote
    /// Used to get fee and amount required for a payment request
    async fn get_payment_quote(
        &self,
        request: &str,
        unit: &CurrencyUnit,
        options: Option<MeltOptions>,
    ) -> Result<PaymentQuoteResponse, Self::Err>;

    /// Pay request
    async fn make_payment(
        &self,
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee_amount: Option<Amount>,
    ) -> Result<MakePaymentResponse, Self::Err>;

    /// Listen for invoices to be paid to the mint
    /// Returns a stream of request_lookup_id once invoices are paid
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err>;

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool;

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self);

    /// Check the status of an incoming payment
    async fn check_incoming_payment_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err>;

    /// Check the status of an outgoing payment
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &str,
    ) -> Result<MakePaymentResponse, Self::Err>;
}

/// Create incoming payment response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIncomingPaymentResponse {
    /// Id that is used to look up the payment from the ln backend
    pub request_lookup_id: String,
    /// Payment request
    pub request: String,
    /// Unix Expiry of Invoice
    pub expiry: Option<u64>,
}

/// Payment response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MakePaymentResponse {
    /// Payment hash
    pub payment_lookup_id: String,
    /// Payment proof
    pub payment_proof: Option<String>,
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

/// Ln backend settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bolt11Settings {
    /// MPP supported
    pub mpp: bool,
    /// Base unit of backend
    pub unit: CurrencyUnit,
    /// Invoice Description supported
    pub invoice_description: bool,
}

impl BaseMintSettings for Bolt11Settings {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl TryFrom<Bolt11Settings> for Value {
    type Error = crate::error::Error;

    fn try_from(value: Bolt11Settings) -> Result<Self, Self::Error> {
        serde_json::to_value(value).map_err(|err| err.into())
    }
}

impl TryFrom<Value> for Bolt11Settings {
    type Error = crate::error::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        serde_json::from_value(value).map_err(|err| err.into())
    }
}
