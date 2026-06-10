//! Exchange-rate oracle primitives for rate-quoted payment processors.

pub mod oracle;
pub mod payment;
pub mod sources;
pub mod store;
pub mod types;

pub use oracle::{AggregatingRateOracle, AggregatorConfig, BackoffState, RateOracle, RateSource};
pub use payment::{
    parked_payment_event_count, PaymentErrorAdapter, RateConvertingPayment,
    RateConvertingPaymentConfig, RateConvertingPaymentError, RateQuoteControlHandle,
    SharedMintPayment, UnitQuoteState, DEFAULT_RATE_QUOTE_TTL_SECS,
};
pub use store::{
    DynRateQuoteStore, InMemoryRateQuoteStore, ParkedPaymentRecord, RateQuoteRecord,
    RateQuoteStore, RateQuoteStoreError,
};
pub use types::{AggregationMeta, RateOracleError, RateSnapshot, SourceReading};
