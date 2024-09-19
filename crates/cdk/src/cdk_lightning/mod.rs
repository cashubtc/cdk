//! CDK Mint Lightning

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use lightning_invoice::{Bolt11Invoice, ParseOrSemanticError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::nuts::{
    CurrencyUnit, MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteState, MintMethodSettings,
    MintQuoteState,
};
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
    /// Cannot convert units
    #[error("Cannot convert units")]
    CannotConvertUnits,
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
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err>;

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
}

/// Create invoice response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInvoiceResponse {
    /// Id that is used to look up the invoice from the ln backend
    pub request_lookup_id: String,
    /// Bolt11 payment request
    pub request: Bolt11Invoice,
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

/// Ln backend settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// MPP supported
    pub mpp: bool,
    /// Min amount to mint
    pub mint_settings: MintMethodSettings,
    /// Max amount to mint
    pub melt_settings: MeltMethodSettings,
    /// Base unit of backend
    pub unit: CurrencyUnit,
    /// Invoice Description supported
    pub invoice_description: bool,
}

/// Msats in sat
pub const MSAT_IN_SAT: u64 = 1000;

/// Helper function to convert units
pub fn to_unit<T>(
    amount: T,
    current_unit: &CurrencyUnit,
    target_unit: &CurrencyUnit,
) -> Result<Amount, Error>
where
    T: Into<u64>,
{
    let amount = amount.into();
    match (current_unit, target_unit) {
        (CurrencyUnit::Sat, CurrencyUnit::Sat) => Ok(amount.into()),
        (CurrencyUnit::Msat, CurrencyUnit::Msat) => Ok(amount.into()),
        (CurrencyUnit::Sat, CurrencyUnit::Msat) => Ok((amount * MSAT_IN_SAT).into()),
        (CurrencyUnit::Msat, CurrencyUnit::Sat) => Ok((amount / MSAT_IN_SAT).into()),
        (CurrencyUnit::Usd, CurrencyUnit::Usd) => Ok(amount.into()),
        (CurrencyUnit::Eur, CurrencyUnit::Eur) => Ok(amount.into()),
        _ => Err(Error::CannotConvertUnits),
    }
}
