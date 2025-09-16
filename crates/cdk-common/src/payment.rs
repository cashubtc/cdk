//! CDK Mint Lightning

use std::convert::Infallible;
use std::pin::Pin;

use async_trait::async_trait;
use cashu::util::hex;
use cashu::{Bolt11Invoice, MeltOptions};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use futures::Stream;
use lightning::offers::offer::Offer;
use lightning_invoice::ParseOrSemanticError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::mint::MeltPaymentRequest;
use crate::nuts::{CurrencyUnit, MeltQuoteState};
use crate::Amount;

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
    /// Amount mismatch
    #[error("Amount is not what is expected")]
    AmountMismatch,
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
    /// NUT23 Error
    #[error(transparent)]
    NUT23(#[from] crate::nuts::nut23::Error),
    /// Hex error
    #[error("Hex error")]
    Hex(#[from] hex::Error),
    /// Invalid hash
    #[error("Invalid hash")]
    InvalidHash,
    /// Custom
    #[error("`{0}`")]
    Custom(String),
}

impl From<Infallible> for Error {
    fn from(_: Infallible) -> Self {
        unreachable!("Infallible cannot be constructed")
    }
}

/// Payment identifier types
#[derive(Debug, Clone, Hash, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum PaymentIdentifier {
    /// Label identifier
    Label(String),
    /// Offer ID identifier
    OfferId(String),
    /// Payment hash identifier
    PaymentHash([u8; 32]),
    /// Bolt12 payment hash
    Bolt12PaymentHash([u8; 32]),
    /// Payment id
    PaymentId([u8; 32]),
    /// Custom Payment ID
    CustomId(String),
}

impl PaymentIdentifier {
    /// Create new [`PaymentIdentifier`]
    pub fn new(kind: &str, identifier: &str) -> Result<Self, Error> {
        match kind.to_lowercase().as_str() {
            "label" => Ok(Self::Label(identifier.to_string())),
            "offer_id" => Ok(Self::OfferId(identifier.to_string())),
            "payment_hash" => Ok(Self::PaymentHash(
                hex::decode(identifier)?
                    .try_into()
                    .map_err(|_| Error::InvalidHash)?,
            )),
            "bolt12_payment_hash" => Ok(Self::Bolt12PaymentHash(
                hex::decode(identifier)?
                    .try_into()
                    .map_err(|_| Error::InvalidHash)?,
            )),
            "custom" => Ok(Self::CustomId(identifier.to_string())),
            "payment_id" => Ok(Self::PaymentId(
                hex::decode(identifier)?
                    .try_into()
                    .map_err(|_| Error::InvalidHash)?,
            )),
            _ => Err(Error::UnsupportedPaymentOption),
        }
    }

    /// Payment id kind
    pub fn kind(&self) -> String {
        match self {
            Self::Label(_) => "label".to_string(),
            Self::OfferId(_) => "offer_id".to_string(),
            Self::PaymentHash(_) => "payment_hash".to_string(),
            Self::Bolt12PaymentHash(_) => "bolt12_payment_hash".to_string(),
            Self::PaymentId(_) => "payment_id".to_string(),
            Self::CustomId(_) => "custom".to_string(),
        }
    }
}

impl std::fmt::Display for PaymentIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Label(l) => write!(f, "{l}"),
            Self::OfferId(o) => write!(f, "{o}"),
            Self::PaymentHash(h) => write!(f, "{}", hex::encode(h)),
            Self::Bolt12PaymentHash(h) => write!(f, "{}", hex::encode(h)),
            Self::PaymentId(h) => write!(f, "{}", hex::encode(h)),
            Self::CustomId(c) => write!(f, "{c}"),
        }
    }
}

/// Options for creating a BOLT11 incoming payment request
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bolt11IncomingPaymentOptions {
    /// Optional description for the payment request
    pub description: Option<String>,
    /// Amount for the payment request in sats
    pub amount: Amount,
    /// Optional expiry time as Unix timestamp in seconds
    pub unix_expiry: Option<u64>,
}

/// Options for creating a BOLT12 incoming payment request
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bolt12IncomingPaymentOptions {
    /// Optional description for the payment request
    pub description: Option<String>,
    /// Optional amount for the payment request in sats
    pub amount: Option<Amount>,
    /// Optional expiry time as Unix timestamp in seconds
    pub unix_expiry: Option<u64>,
}

/// Options for creating an incoming payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IncomingPaymentOptions {
    /// BOLT11 payment request options
    Bolt11(Bolt11IncomingPaymentOptions),
    /// BOLT12 payment request options
    Bolt12(Box<Bolt12IncomingPaymentOptions>),
}

/// Options for BOLT11 outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bolt11OutgoingPaymentOptions {
    /// Bolt11
    pub bolt11: Bolt11Invoice,
    /// Maximum fee amount allowed for the payment
    pub max_fee_amount: Option<Amount>,
    /// Optional timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Melt options
    pub melt_options: Option<MeltOptions>,
}

/// Options for BOLT12 outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bolt12OutgoingPaymentOptions {
    /// Offer
    pub offer: Offer,
    /// Maximum fee amount allowed for the payment
    pub max_fee_amount: Option<Amount>,
    /// Optional timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Melt options
    pub melt_options: Option<MeltOptions>,
}

/// Options for creating an outgoing payment
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutgoingPaymentOptions {
    /// BOLT11 payment options
    Bolt11(Box<Bolt11OutgoingPaymentOptions>),
    /// BOLT12 payment options
    Bolt12(Box<Bolt12OutgoingPaymentOptions>),
}

impl TryFrom<crate::mint::MeltQuote> for OutgoingPaymentOptions {
    type Error = Error;

    fn try_from(melt_quote: crate::mint::MeltQuote) -> Result<Self, Self::Error> {
        match melt_quote.request {
            MeltPaymentRequest::Bolt11 { bolt11 } => Ok(OutgoingPaymentOptions::Bolt11(Box::new(
                Bolt11OutgoingPaymentOptions {
                    max_fee_amount: Some(melt_quote.fee_reserve),
                    timeout_secs: None,
                    bolt11,
                    melt_options: melt_quote.options,
                },
            ))),
            MeltPaymentRequest::Bolt12 { offer } => {
                let melt_options = match melt_quote.options {
                    None => None,
                    Some(MeltOptions::Mpp { mpp: _ }) => return Err(Error::UnsupportedUnit),
                    Some(options) => Some(options),
                };

                Ok(OutgoingPaymentOptions::Bolt12(Box::new(
                    Bolt12OutgoingPaymentOptions {
                        max_fee_amount: Some(melt_quote.fee_reserve),
                        timeout_secs: None,
                        offer: *offer,
                        melt_options,
                    },
                )))
            }
        }
    }
}

/// Mint payment trait
#[async_trait]
pub trait MintPayment {
    /// Mint Lightning Error
    type Err: Into<Error> + From<Error>;

    /// Start the payment processor
    /// Called when the mint starts up to initialize the payment processor
    async fn start(&self) -> Result<(), Self::Err> {
        // Default implementation - do nothing
        Ok(())
    }

    /// Stop the payment processor
    /// Called when the mint shuts down to gracefully stop the payment processor
    async fn stop(&self) -> Result<(), Self::Err> {
        // Default implementation - do nothing
        Ok(())
    }

    /// Base Settings
    async fn get_settings(&self) -> Result<serde_json::Value, Self::Err>;

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err>;

    /// Get payment quote
    /// Used to get fee and amount required for a payment request
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err>;

    /// Pay request
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err>;

    /// Listen for invoices to be paid to the mint
    /// Returns a stream of request_lookup_id once invoices are paid
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err>;

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool;

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self);

    /// Check the status of an incoming payment
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err>;

    /// Check the status of an outgoing payment
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err>;
}

/// An event emitted which should be handled by the mint
#[derive(Debug, Clone, Hash)]
pub enum Event {
    /// A payment has been received.
    PaymentReceived(WaitPaymentResponse),
}

impl Default for Event {
    fn default() -> Self {
        // We use this as a sentinel value for no-op events
        // The actual processing will filter these out
        Event::PaymentReceived(WaitPaymentResponse {
            payment_identifier: PaymentIdentifier::CustomId("default".to_string()),
            payment_amount: Amount::from(0),
            unit: CurrencyUnit::Msat,
            payment_id: "default".to_string(),
        })
    }
}

/// Wait any invoice response
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub struct WaitPaymentResponse {
    /// Request look up id
    /// Id that relates the quote and payment request
    pub payment_identifier: PaymentIdentifier,
    /// Payment amount
    pub payment_amount: Amount,
    /// Unit
    pub unit: CurrencyUnit,
    /// Unique id of payment
    // Payment hash
    pub payment_id: String,
}

/// Create incoming payment response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateIncomingPaymentResponse {
    /// Id that is used to look up the payment from the ln backend
    pub request_lookup_id: PaymentIdentifier,
    /// Payment request
    pub request: String,
    /// Unix Expiry of Invoice
    pub expiry: Option<u64>,
}

/// Payment response
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MakePaymentResponse {
    /// Payment hash
    pub payment_lookup_id: PaymentIdentifier,
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
    pub request_lookup_id: Option<PaymentIdentifier>,
    /// Amount
    pub amount: Amount,
    /// Fee required for melt
    pub fee: Amount,
    /// Currency unit of `amount` and `fee`
    pub unit: CurrencyUnit,
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
    /// Paying amountless invoices supported
    pub amountless: bool,
    /// Bolt12 supported
    pub bolt12: bool,
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

/// Metrics wrapper for MintPayment implementations
///
/// This wrapper implements the Decorator pattern to collect metrics on all
/// MintPayment trait methods. It wraps any existing MintPayment implementation
/// and automatically records timing and operation metrics.
#[derive(Clone)]
#[cfg(feature = "prometheus")]
pub struct MetricsMintPayment<T> {
    inner: T,
}
#[cfg(feature = "prometheus")]
impl<T> MetricsMintPayment<T>
where
    T: MintPayment,
{
    /// Create a new metrics wrapper around a MintPayment implementation
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Get reference to the underlying implementation
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Consume the wrapper and return the inner implementation
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[async_trait]
#[cfg(feature = "prometheus")]
impl<T> MintPayment for MetricsMintPayment<T>
where
    T: MintPayment + Send + Sync,
{
    type Err = T::Err;

    async fn get_settings(&self) -> Result<serde_json::Value, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("get_settings");

        let result = self.inner.get_settings().await;

        let duration = start.elapsed().as_secs_f64();
        METRICS.record_mint_operation_histogram("get_settings", result.is_ok(), duration);
        METRICS.dec_in_flight_requests("get_settings");

        result
    }

    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("create_incoming_payment_request");

        let result = self
            .inner
            .create_incoming_payment_request(unit, options)
            .await;

        let duration = start.elapsed().as_secs_f64();
        METRICS.record_mint_operation_histogram(
            "create_incoming_payment_request",
            result.is_ok(),
            duration,
        );
        METRICS.dec_in_flight_requests("create_incoming_payment_request");

        result
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("get_payment_quote");

        let result = self.inner.get_payment_quote(unit, options).await;

        let duration = start.elapsed().as_secs_f64();
        let success = result.is_ok();

        if let Ok(ref quote) = result {
            let amount: f64 = u64::from(quote.amount) as f64;
            let fee: f64 = u64::from(quote.fee) as f64;
            METRICS.record_lightning_payment(amount, fee);
        }

        METRICS.record_mint_operation_histogram("get_payment_quote", success, duration);
        METRICS.dec_in_flight_requests("get_payment_quote");

        result
    }
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("wait_payment_event");

        let result = self.inner.wait_payment_event().await;

        let duration = start.elapsed().as_secs_f64();
        let success = result.is_ok();

        METRICS.record_mint_operation_histogram("wait_payment_event", success, duration);
        METRICS.dec_in_flight_requests("wait_payment_event");

        result
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("make_payment");

        let result = self.inner.make_payment(unit, options).await;

        let duration = start.elapsed().as_secs_f64();
        let success = result.is_ok();

        METRICS.record_mint_operation_histogram("make_payment", success, duration);
        METRICS.dec_in_flight_requests("make_payment");

        result
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.inner.is_wait_invoice_active()
    }

    fn cancel_wait_invoice(&self) {
        self.inner.cancel_wait_invoice()
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("check_incoming_payment_status");

        let result = self
            .inner
            .check_incoming_payment_status(payment_identifier)
            .await;

        let duration = start.elapsed().as_secs_f64();
        METRICS.record_mint_operation_histogram(
            "check_incoming_payment_status",
            result.is_ok(),
            duration,
        );
        METRICS.dec_in_flight_requests("check_incoming_payment_status");

        result
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("check_outgoing_payment");

        let result = self.inner.check_outgoing_payment(payment_identifier).await;

        let duration = start.elapsed().as_secs_f64();
        let success = result.is_ok();

        METRICS.record_mint_operation_histogram("check_outgoing_payment", success, duration);
        METRICS.dec_in_flight_requests("check_outgoing_payment");

        result
    }
}

/// Type alias for Mint Payment trait
pub type DynMintPayment = std::sync::Arc<dyn MintPayment<Err = Error> + Send + Sync>;
