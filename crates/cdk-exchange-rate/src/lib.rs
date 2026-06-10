//! Exchange-rate oracle primitives for rate-quoted payment processors.

pub mod oracle;
pub mod sources;
pub mod types;

pub use oracle::{AggregatingRateOracle, AggregatorConfig, BackoffState, RateOracle, RateSource};
pub use types::{AggregationMeta, RateOracleError, RateSnapshot, SourceReading};
