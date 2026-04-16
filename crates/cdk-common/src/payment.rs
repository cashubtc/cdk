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

use crate::mint::{MeltPaymentRequest, MeltQuote};
use crate::nuts::{CurrencyUnit, MeltQuoteState};
use crate::{Amount, QuoteId};

/// CDK Payment Error
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
    /// Invalid expiry
    #[error("Invalid expiry")]
    InvalidExpiry,
    /// Lightning Error
    #[error(transparent)]
    Lightning(Box<dyn std::error::Error + Send + Sync>),
    /// Onchain Error
    #[error(transparent)]
    Onchain(Box<dyn std::error::Error + Send + Sync>),
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
#[derive(Clone, Hash, PartialEq, Eq, Deserialize, Serialize)]
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
    /// Quote ID
    QuoteId(QuoteId),
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
            "quote_id" => {
                Ok(Self::QuoteId(identifier.parse().map_err(|_| {
                    Error::Custom("Invalid QuoteId".to_string())
                })?))
            }
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
            Self::QuoteId(_) => "quote_id".to_string(),
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
            Self::QuoteId(q) => write!(f, "{q}"),
        }
    }
}

impl std::fmt::Debug for PaymentIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaymentIdentifier::PaymentHash(h) => write!(f, "PaymentHash({})", hex::encode(h)),
            PaymentIdentifier::Bolt12PaymentHash(h) => {
                write!(f, "Bolt12PaymentHash({})", hex::encode(h))
            }
            PaymentIdentifier::PaymentId(h) => write!(f, "PaymentId({})", hex::encode(h)),
            PaymentIdentifier::Label(s) => write!(f, "Label({})", s),
            PaymentIdentifier::OfferId(s) => write!(f, "OfferId({})", s),
            PaymentIdentifier::CustomId(s) => write!(f, "CustomId({})", s),
            PaymentIdentifier::QuoteId(q) => write!(f, "QuoteId({})", q),
        }
    }
}

/// Options for creating a BOLT11 incoming payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bolt11IncomingPaymentOptions {
    /// Optional description for the payment request
    pub description: Option<String>,
    /// Amount for the payment request in sats
    pub amount: Amount<CurrencyUnit>,
    /// Optional expiry time as Unix timestamp in seconds
    pub unix_expiry: Option<u64>,
}

impl Default for Bolt11IncomingPaymentOptions {
    fn default() -> Self {
        Self {
            description: None,
            amount: Amount::new(0, CurrencyUnit::Sat),
            unix_expiry: None,
        }
    }
}

/// Options for creating a BOLT12 incoming payment request
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bolt12IncomingPaymentOptions {
    /// Optional description for the payment request
    pub description: Option<String>,
    /// Optional amount for the payment request in sats
    pub amount: Option<Amount<CurrencyUnit>>,
    /// Optional expiry time as Unix timestamp in seconds
    pub unix_expiry: Option<u64>,
}

/// Options for creating a custom incoming payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CustomIncomingPaymentOptions {
    /// Payment method name (e.g., "paypal", "venmo")
    pub method: String,
    /// Optional description for the payment request
    pub description: Option<String>,
    /// Amount for the payment request
    pub amount: Amount<CurrencyUnit>,
    /// Optional expiry time as Unix timestamp in seconds
    pub unix_expiry: Option<u64>,
    /// Extra payment-method-specific fields as JSON string
    ///
    /// These fields are passed through to the payment processor for
    /// method-specific validation (e.g., ehash share).
    pub extra_json: Option<String>,
}

/// Options for creating an onchain incoming payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OnchainIncomingPaymentOptions {
    /// Quote ID for the incoming payment
    pub quote_id: QuoteId,
}

/// Options for incoming payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IncomingPaymentOptions {
    /// BOLT11 payment request options
    Bolt11(Bolt11IncomingPaymentOptions),
    /// BOLT12 payment request options
    Bolt12(Box<Bolt12IncomingPaymentOptions>),
    /// Custom payment method options
    Custom(Box<CustomIncomingPaymentOptions>),
    /// Onchain payment request options
    Onchain(OnchainIncomingPaymentOptions),
}

/// Options for BOLT11 outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bolt11OutgoingPaymentOptions {
    /// Bolt11
    pub bolt11: Bolt11Invoice,
    /// Maximum fee amount allowed for the payment
    pub max_fee_amount: Option<Amount<CurrencyUnit>>,
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
    pub max_fee_amount: Option<Amount<CurrencyUnit>>,
    /// Optional timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Melt options
    pub melt_options: Option<MeltOptions>,
}

/// Options for custom outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CustomOutgoingPaymentOptions {
    /// Payment method name
    pub method: String,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Maximum fee amount allowed for the payment
    pub max_fee_amount: Option<Amount<CurrencyUnit>>,
    /// Optional timeout in seconds
    pub timeout_secs: Option<u64>,
    /// Melt options
    pub melt_options: Option<MeltOptions>,
    /// Extra payment-method-specific fields as JSON string
    ///
    /// These fields are passed through to the payment processor for
    /// method-specific validation.
    pub extra_json: Option<String>,
}

/// Options for onchain outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OnchainOutgoingPaymentOptions {
    /// Bitcoin address to send to
    pub address: String,
    /// Payment amount
    pub amount: Amount<CurrencyUnit>,
    /// Maximum fee amount allowed for the payment
    pub max_fee_amount: Option<Amount<CurrencyUnit>>,
    /// Opaque stable identifier supplied by the mint.
    ///
    /// The mint generates this value and uses it to correlate the quote with
    /// subsequent `make_payment` and `check_outgoing_payment` calls. Backends
    /// MUST NOT synthesize or modify this value. Backends MUST persist it
    /// (for example as the send intent id) and echo it verbatim in
    /// [`PaymentQuoteResponse::request_lookup_id`] and
    /// [`MakePaymentResponse::payment_lookup_id`] as
    /// `PaymentIdentifier::QuoteId(..)`. The mint layer validates the echo
    /// and will reject quotes whose backend response disagrees with the
    /// supplied `quote_id` (see
    /// [`Error::OnchainQuoteLookupIdMismatch`](crate::Error::OnchainQuoteLookupIdMismatch)).
    pub quote_id: QuoteId,
    /// Batching tier hint (e.g. "immediate", "standard", "economy")
    pub tier: Option<String>,
    /// Opaque metadata as a JSON string for future extensions
    pub metadata: Option<String>,
}

/// Options for outgoing payments
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutgoingPaymentOptions {
    /// BOLT11 payment options
    Bolt11(Box<Bolt11OutgoingPaymentOptions>),
    /// BOLT12 payment options
    Bolt12(Box<Bolt12OutgoingPaymentOptions>),
    /// Custom payment method options
    Custom(Box<CustomOutgoingPaymentOptions>),
    /// Onchain payment options
    Onchain(Box<OnchainOutgoingPaymentOptions>),
}

impl OutgoingPaymentOptions {
    /// Creates payment options from a melt quote
    pub fn from_melt_quote_with_fee(
        melt_quote: MeltQuote,
    ) -> Result<OutgoingPaymentOptions, Error> {
        let fee_reserve = melt_quote.fee_reserve();
        match &melt_quote.request {
            MeltPaymentRequest::Bolt11 { bolt11 } => Ok(OutgoingPaymentOptions::Bolt11(Box::new(
                Bolt11OutgoingPaymentOptions {
                    max_fee_amount: Some(fee_reserve),
                    timeout_secs: None,
                    bolt11: bolt11.clone(),
                    melt_options: melt_quote.options,
                },
            ))),
            MeltPaymentRequest::Bolt12 { offer } => {
                let melt_options = match melt_quote.options {
                    Some(MeltOptions::Mpp { mpp: _ }) => return Err(Error::UnsupportedUnit),
                    Some(options) => Some(options),
                    _ => None,
                };

                Ok(OutgoingPaymentOptions::Bolt12(Box::new(
                    Bolt12OutgoingPaymentOptions {
                        max_fee_amount: Some(fee_reserve),
                        timeout_secs: None,
                        offer: *offer.clone(),
                        melt_options,
                    },
                )))
            }
            MeltPaymentRequest::Custom { method, request } => Ok(OutgoingPaymentOptions::Custom(
                Box::new(CustomOutgoingPaymentOptions {
                    method: method.to_string(),
                    request: request.to_string(),
                    max_fee_amount: Some(fee_reserve),
                    timeout_secs: None,
                    melt_options: melt_quote.options,
                    extra_json: None,
                }),
            )),
            MeltPaymentRequest::Onchain { address } => Ok(OutgoingPaymentOptions::Onchain(
                Box::new(OnchainOutgoingPaymentOptions {
                    address: address.clone(),
                    amount: melt_quote.amount(),
                    max_fee_amount: Some(fee_reserve),
                    quote_id: melt_quote.id,
                    // TODO(#TBD): Propagate tier and metadata from MeltQuote
                    // once the quote struct carries them. Hard-wired to None
                    // today because MeltQuote has no tier/metadata fields, so
                    // cdk-bdk's Standard/Economy batching is unreachable via
                    // the standard melt flow. Load-bearing pair of this site:
                    // crates/cdk/src/mint/melt/mod.rs (get_melt_onchain_quote_impl).
                    tier: None,
                    metadata: None,
                }),
            )),
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
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err>;

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
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

    /// Is the payment event stream active
    fn is_payment_event_stream_active(&self) -> bool;

    /// Cancel the payment event stream
    fn cancel_payment_event_stream(&self);

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
    /// An outgoing payment has been confirmed.
    PaymentSuccessful {
        /// Quote ID linking to the melt quote
        quote_id: QuoteId,
        /// Payment response details
        details: MakePaymentResponse,
    },
    /// An outgoing payment has permanently failed.
    PaymentFailed {
        /// Quote ID linking to the melt quote
        quote_id: QuoteId,
        /// Human-readable reason for the failure
        reason: String,
    },
}

/// Wait any invoice response
#[derive(Debug, Clone, Hash)]
pub struct WaitPaymentResponse {
    /// Request look up id
    /// Id that relates the quote and payment request
    pub payment_identifier: PaymentIdentifier,
    /// Payment amount (typed with unit for compile-time safety)
    pub payment_amount: Amount<CurrencyUnit>,
    /// Unique id of payment
    // Payment hash
    pub payment_id: String,
}

impl WaitPaymentResponse {
    /// Get the currency unit
    pub fn unit(&self) -> &CurrencyUnit {
        self.payment_amount.unit()
    }
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
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    #[serde(flatten, default)]
    pub extra_json: Option<serde_json::Value>,
}

/// Payment response
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct MakePaymentResponse {
    /// Payment hash
    ///
    /// For onchain payments, this MUST be
    /// `PaymentIdentifier::QuoteId(quote_id)` where `quote_id` is the value
    /// supplied by the mint in
    /// [`OnchainOutgoingPaymentOptions::quote_id`]. See that field for the
    /// full echo contract.
    pub payment_lookup_id: PaymentIdentifier,
    /// Payment proof
    pub payment_proof: Option<String>,
    /// Status
    pub status: MeltQuoteState,
    /// Total amount spent, including fees. Only authoritative when `status`
    /// is [`MeltQuoteState::Paid`]; otherwise backends return `0`.
    pub total_spent: Amount<CurrencyUnit>,
}

impl MakePaymentResponse {
    /// Get the currency unit
    pub fn unit(&self) -> &CurrencyUnit {
        self.total_spent.unit()
    }
}

/// Payment quote response
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct PaymentQuoteResponse {
    /// Request look up id
    ///
    /// For onchain quotes, this MUST be
    /// `Some(PaymentIdentifier::QuoteId(quote_id))` where `quote_id` is the
    /// value supplied by the mint in
    /// [`OnchainOutgoingPaymentOptions::quote_id`]. The mint validates this
    /// echo and rejects mismatches — see
    /// [`OnchainOutgoingPaymentOptions::quote_id`] for the full contract.
    pub request_lookup_id: Option<PaymentIdentifier>,
    /// Amount (typed with unit for compile-time safety)
    pub amount: Amount<CurrencyUnit>,
    /// Fee required for melt (typed with unit for compile-time safety)
    pub fee: Amount<CurrencyUnit>,
    /// Status
    pub state: MeltQuoteState,
    /// Extra payment-method-specific fields
    pub extra_json: Option<serde_json::Value>,
    /// Estimated confirmation target in blocks for onchain quotes
    pub estimated_blocks: Option<u32>,
}

impl PaymentQuoteResponse {
    /// Get the currency unit
    pub fn unit(&self) -> &CurrencyUnit {
        self.amount.unit()
    }
}

/// BOLT11 settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Bolt11Settings {
    /// Multi-part payment (MPP) supported
    pub mpp: bool,
    /// Amountless invoice support
    pub amountless: bool,
    /// Invoice description supported
    pub invoice_description: bool,
}

/// BOLT12 settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Bolt12Settings {
    /// Amountless offer support
    pub amountless: bool,
}

/// Onchain settings
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OnchainSettings {
    /// Number of confirmations required
    pub confirmations: u32,
    /// Minimum incoming onchain payment amount accepted by the backend
    pub min_receive_amount_sat: u64,
}

/// Payment processor settings response
/// Mirrors the proto SettingsResponse structure
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsResponse {
    /// Base unit of backend
    pub unit: String,
    /// BOLT11 settings (None if not supported)
    pub bolt11: Option<Bolt11Settings>,
    /// BOLT12 settings (None if not supported)
    pub bolt12: Option<Bolt12Settings>,
    /// Onchain settings (None if not supported)
    pub onchain: Option<OnchainSettings>,
    /// Custom payment methods settings (method name -> settings data)
    #[serde(default)]
    pub custom: std::collections::HashMap<String, String>,
}

impl From<SettingsResponse> for Value {
    fn from(value: SettingsResponse) -> Self {
        serde_json::to_value(value).unwrap_or(Value::Null)
    }
}

impl TryFrom<Value> for SettingsResponse {
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
#[derive(Debug, Clone)]
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

    async fn start(&self) -> Result<(), Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("start");

        let result = self.inner.start().await;

        let duration = start.elapsed().as_secs_f64();
        METRICS.record_mint_operation_histogram("start", result.is_ok(), duration);
        METRICS.dec_in_flight_requests("start");

        result
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("stop");

        let result = self.inner.stop().await;

        let duration = start.elapsed().as_secs_f64();
        METRICS.record_mint_operation_histogram("stop", result.is_ok(), duration);
        METRICS.dec_in_flight_requests("stop");

        result
    }
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
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
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let start = std::time::Instant::now();
        METRICS.inc_in_flight_requests("create_incoming_payment_request");

        let result = self.inner.create_incoming_payment_request(options).await;

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
            let amount: f64 = quote.amount.value() as f64;
            let fee: f64 = quote.fee.value() as f64;
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

    fn is_payment_event_stream_active(&self) -> bool {
        self.inner.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream()
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::QuoteId;

    #[test]
    fn test_payment_identifier_quote_id_roundtrip() {
        let quote_id = QuoteId::new_uuid();
        let identifier = PaymentIdentifier::QuoteId(quote_id.clone());

        let kind = identifier.kind();
        assert_eq!(kind, "quote_id");

        let display = identifier.to_string();
        assert_eq!(display, quote_id.to_string());

        let debug = format!("{:?}", identifier);
        assert_eq!(debug, format!("QuoteId({})", quote_id));

        let parsed = PaymentIdentifier::new(&kind, &display).unwrap();
        assert_eq!(parsed, identifier);
    }

    #[test]
    fn test_payment_identifier_quote_id_base64_roundtrip() {
        let quote_id_str = "SGVsbG8gV29ybGQh"; // Valid Base64
        let identifier = PaymentIdentifier::QuoteId(QuoteId::from_str(quote_id_str).unwrap());

        let kind = identifier.kind();
        assert_eq!(kind, "quote_id");

        let display = identifier.to_string();
        assert_eq!(display, quote_id_str);

        let parsed = PaymentIdentifier::new(&kind, &display).unwrap();
        assert_eq!(parsed, identifier);
    }

    #[test]
    fn test_payment_identifier_unsupported_kind() {
        let result = PaymentIdentifier::new("unsupported_kind", "123");
        assert!(matches!(result, Err(Error::UnsupportedPaymentOption)));
    }

    #[test]
    fn test_payment_identifier_invalid_quote_id() {
        // An invalid base64 and invalid UUID string (e.g. spaces and special characters)
        let result = PaymentIdentifier::new("quote_id", "invalid!@#quote");
        assert!(matches!(result, Err(Error::Custom(_))));
    }

    #[test]
    fn test_payment_identifier_invalid_hash() {
        // Invalid hex
        let result_hex = PaymentIdentifier::new("payment_hash", "not_hex!");
        assert!(matches!(result_hex, Err(Error::Hex(_))));

        // Valid hex, but wrong length (e.g. 1 byte instead of 32)
        let result_len = PaymentIdentifier::new("payment_hash", "00");
        assert!(matches!(result_len, Err(Error::InvalidHash)));

        // Invalid length for bolt12_payment_hash
        let result_bolt12 = PaymentIdentifier::new("bolt12_payment_hash", "00");
        assert!(matches!(result_bolt12, Err(Error::InvalidHash)));
    }
}

#[test]
fn test_payment_identifier_hash_variants_roundtrip() {
    let dummy_hash = [1u8; 32];
    let hex_encoded = hex::encode(dummy_hash);

    // Test Bolt12PaymentHash
    let bolt12_identifier = PaymentIdentifier::Bolt12PaymentHash(dummy_hash);

    let kind = bolt12_identifier.kind();
    assert_eq!(kind, "bolt12_payment_hash");

    let display = bolt12_identifier.to_string();
    assert_eq!(display, hex_encoded);

    let debug = format!("{:?}", bolt12_identifier);
    assert_eq!(debug, format!("Bolt12PaymentHash({})", hex_encoded));

    let parsed = PaymentIdentifier::new(&kind, &display).unwrap();
    assert_eq!(parsed, bolt12_identifier);

    // Test PaymentId
    let dummy_hash_2 = [2u8; 32];
    let hex_encoded_2 = hex::encode(dummy_hash_2);
    let payment_id_identifier = PaymentIdentifier::PaymentId(dummy_hash_2);

    let kind = payment_id_identifier.kind();
    assert_eq!(kind, "payment_id");

    let display = payment_id_identifier.to_string();
    assert_eq!(display, hex_encoded_2);

    let debug = format!("{:?}", payment_id_identifier);
    assert_eq!(debug, format!("PaymentId({})", hex_encoded_2));

    let parsed = PaymentIdentifier::new(&kind, &display).unwrap();
    assert_eq!(parsed, payment_id_identifier);
}
