//! Shared exchange-rate oracle types.

use std::time::SystemTime;

use cdk_common::nuts::CurrencyUnit;

/// A successful exchange-rate snapshot for one fiat currency unit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RateSnapshot {
    /// Fiat unit requested for the snapshot.
    pub fiat: CurrencyUnit,
    /// Aggregated rate in sats per fiat unit.
    pub aggregated_rate: f64,
    /// Individual source readings considered by the oracle.
    pub source_readings: Vec<SourceReading>,
    /// Metadata describing the aggregation decision.
    pub aggregation_meta: AggregationMeta,
    /// Local wall-clock time when the snapshot was created.
    pub created_at: SystemTime,
}

/// One successful source reading and its aggregation decision.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceReading {
    /// Human-readable source name.
    pub source_name: String,
    /// Source rate in sats per fiat unit.
    pub rate: f64,
    /// Age measured from the local monotonic clock at fetch completion.
    pub fetched_at_age_secs: u64,
    /// Optional timestamp reported by the source itself.
    pub source_reported_timestamp: Option<SystemTime>,
    /// Whether this reading survived trimming and contributed to the final mean.
    pub included_in_aggregation: bool,
}

/// Metadata for the trimmed-median aggregation process.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AggregationMeta {
    /// Number of successful, non-stale readings available before trimming.
    pub sources_fetched: usize,
    /// Number of readings excluded by deviation trimming.
    pub sources_trimmed: usize,
    /// Number of readings that contributed to the final mean.
    pub sources_survived: usize,
    /// Median rate before deviation trimming.
    pub median_before_trim: f64,
    /// Deviation threshold used for trimming, as a percentage.
    pub deviation_threshold_pct: f64,
}

/// Errors returned by exchange-rate oracles.
#[derive(Debug, thiserror::Error)]
pub enum RateOracleError {
    /// All sources were stale or timed out.
    #[error("all sources stale or timed out")]
    AllStale,
    /// Fewer than the required source count were available.
    #[error("insufficient sources: fetched {fetched}, required {required}")]
    InsufficientSources {
        /// Number of sources fetched or surviving the current aggregation phase.
        fetched: usize,
        /// Required source count for that phase.
        required: usize,
    },
    /// Sources diverged beyond the configured threshold.
    #[error("sources diverge beyond threshold: max deviation {max_deviation_pct:.2}%")]
    Divergence {
        /// Maximum observed percentage deviation from the median.
        max_deviation_pct: f64,
    },
    /// A source returned an error or malformed data.
    #[error("source error: {0}")]
    SourceError(String),
}
