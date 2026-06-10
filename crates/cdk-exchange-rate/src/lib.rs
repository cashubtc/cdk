//! Exchange-rate oracle primitives for rate-quoted payment processors.

pub mod oracle;
pub mod payment;
pub mod sources;
pub mod store;
pub mod types;

pub use oracle::{AggregatingRateOracle, AggregatorConfig, BackoffState, RateOracle, RateSource};
pub use payment::{
    RateConvertingPayment, RateConvertingPaymentConfig, RateConvertingPaymentError,
    DEFAULT_RATE_QUOTE_TTL_SECS,
};
pub use store::{
    DynRateQuoteStore, InMemoryRateQuoteStore, ParkedPaymentRecord, RateQuoteRecord,
    RateQuoteStore, RateQuoteStoreError,
};
pub use types::{AggregationMeta, RateOracleError, RateSnapshot, SourceReading};
