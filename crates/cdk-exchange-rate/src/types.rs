//! Shared exchange-rate oracle types.
//!
//! # Rate contract
//!
//! Every rate in this crate ([`RateSnapshot::aggregated_rate`],
//! [`SourceReading::rate`], and the values returned by
//! [`RateSource::fetch`](crate::oracle::RateSource::fetch)) is expressed in
//! **sats per WHOLE fiat unit** (e.g. sats per US dollar — NOT sats per
//! cent). Quote amounts on the wire are expressed in the unit's minor
//! subunits (USD cents). Conversions between the two divide or multiply by
//! [`fiat_subunit_scale`] (USD = 100):
//!
//! - mint (fiat → sats, mint-favoring round up):
//!   `sats = ceil(fiat_subunits × rate × (1 + buffer) / scale)`
//! - melt (sats → fiat, mint-favoring round up):
//!   `fiat_subunits = ceil(sats × scale × (1 + buffer) / rate)`

use std::time::SystemTime;

use cdk_common::nuts::CurrencyUnit;

/// Return the number of fiat subunits in one whole unit.
pub fn fiat_subunit_scale(unit: &CurrencyUnit) -> Option<u64> {
    match unit {
        CurrencyUnit::Usd => Some(100),
        CurrencyUnit::Custom(s) if s.eq_ignore_ascii_case("milli-cent") => Some(100_000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn milli_cent_custom_unit_uses_one_hundred_thousand_subunits() {
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::Custom("milli-cent".to_string())),
            Some(100_000)
        );
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::Custom("Milli-Cent".to_string())),
            Some(100_000)
        );
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::Custom("MILLI-CENT".to_string())),
            Some(100_000)
        );
    }

    #[test]
    fn milli_cent_custom_unit_does_not_trim_direct_custom_value() {
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::Custom("  milli-cent  ".to_string())),
            None
        );
    }

    #[test]
    fn milli_cent_from_str_trims_and_matches() {
        assert_eq!(
            fiat_subunit_scale(
                &CurrencyUnit::from_str("  milli-cent  ")
                    .expect("custom currency unit should parse")
            ),
            Some(100_000)
        );
    }

    #[test]
    fn millicent_without_hyphen_does_not_match() {
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::Custom("millicent".to_string())),
            None
        );
    }

    #[test]
    fn milli_cent_custom_constructor_uppercase_matches() {
        assert_eq!(
            fiat_subunit_scale(&CurrencyUnit::custom("milli-cent")),
            Some(100_000)
        );
    }
}

/// A successful exchange-rate snapshot for one fiat currency unit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RateSnapshot {
    /// Fiat unit requested for the snapshot.
    pub fiat: CurrencyUnit,
    /// Aggregated rate in sats per whole fiat unit.
    pub aggregated_rate: u64,
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
    /// Source rate in sats per whole fiat unit.
    pub rate: u64,
    /// Age measured from the local monotonic clock at fetch completion.
    pub fetched_at_age_secs: u64,
    /// Optional timestamp reported by the source itself.
    pub source_reported_timestamp: Option<SystemTime>,
    /// Whether this reading survived trimming and contributed to the final median.
    pub included_in_aggregation: bool,
}

/// Metadata for the trimmed-median aggregation process.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AggregationMeta {
    /// Number of successful, non-stale readings available before trimming.
    pub sources_fetched: usize,
    /// Number of readings excluded by deviation trimming.
    pub sources_trimmed: usize,
    /// Number of readings that contributed to the final median.
    pub sources_survived: usize,
    /// Median rate before deviation trimming, in sats per whole fiat unit.
    pub median_before_trim: u64,
    /// Deviation threshold used for trimming, in basis points.
    pub deviation_threshold_bps: u64,
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
    #[error("sources diverge beyond threshold: max deviation {max_deviation_bps} bps")]
    Divergence {
        /// Maximum observed basis-point deviation from the median.
        max_deviation_bps: u64,
    },
    /// A source returned an error or malformed data.
    #[error("source error: {0}")]
    SourceError(String),
}
