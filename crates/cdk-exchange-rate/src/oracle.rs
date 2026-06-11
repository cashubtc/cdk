//! Aggregating exchange-rate oracle implementation.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use futures::future::join_all;
use tokio::time;

use crate::types::{AggregationMeta, RateOracleError, RateSnapshot, SourceReading};

/// Oracle interface for retrieving fiat-denominated rate snapshots.
#[async_trait]
pub trait RateOracle: Send + Sync {
    /// Return a fresh or cached rate snapshot for the requested fiat unit.
    async fn snapshot(&self, fiat: &CurrencyUnit) -> Result<RateSnapshot, RateOracleError>;
}

/// Exchange-rate source interface used by aggregating oracles.
#[async_trait]
pub trait RateSource: Send + Sync {
    /// Human-readable source name.
    fn name(&self) -> &str;

    /// Fetch a rate in sats per whole fiat unit and an optional source-reported timestamp.
    async fn fetch(
        &self,
        fiat: &CurrencyUnit,
    ) -> Result<(u64, Option<SystemTime>), RateOracleError>;
}

/// Configuration for [`AggregatingRateOracle`].
#[derive(Debug, Clone)]
pub struct AggregatorConfig {
    /// Minimum successful, non-stale source count before trimming.
    pub min_sources: usize,
    /// Minimum surviving source count after trimming.
    pub min_survived: usize,
    /// Maximum allowed basis-point deviation from the median.
    pub deviation_threshold_bps: u64,
    /// Per-source fetch timeout in seconds.
    pub fetch_timeout_secs: u64,
    /// Snapshot cache TTL in seconds.
    pub cache_ttl_secs: u64,
    /// Maximum tolerated source-reported clock offset in seconds.
    pub max_clock_offset_secs: u64,
}

impl Default for AggregatorConfig {
    fn default() -> Self {
        Self {
            min_sources: 3,
            min_survived: 2,
            deviation_threshold_bps: 100,
            fetch_timeout_secs: 5,
            cache_ttl_secs: 30,
            max_clock_offset_secs: 30,
        }
    }
}

/// Per-source backoff accounting.
#[derive(Debug, Clone, Default)]
pub struct BackoffState {
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
}

impl BackoffState {
    /// Number of consecutive failures recorded for this source and unit.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Local monotonic instant of the most recent failure.
    pub fn last_failure_at(&self) -> Option<Instant> {
        self.last_failure_at
    }
}

#[derive(Debug, Clone)]
struct CachedSnapshot {
    snapshot: RateSnapshot,
    cached_at: Instant,
}

/// Multi-source trimmed-median rate oracle.
pub struct AggregatingRateOracle {
    sources: Vec<Box<dyn RateSource>>,
    config: AggregatorConfig,
    cache: tokio::sync::Mutex<HashMap<CurrencyUnit, CachedSnapshot>>,
    backoff_state: tokio::sync::Mutex<HashMap<(String, CurrencyUnit), BackoffState>>,
    /// Single-flight guard: serializes cache refreshes so concurrent quote
    /// requests do not fan out duplicate source fetches.
    refresh: tokio::sync::Mutex<()>,
}

impl fmt::Debug for AggregatingRateOracle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AggregatingRateOracle")
            .field("source_count", &self.sources.len())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl AggregatingRateOracle {
    /// Create an oracle with default aggregation configuration.
    pub fn new(sources: Vec<Box<dyn RateSource>>) -> Self {
        Self::with_config(sources, AggregatorConfig::default())
    }

    /// Create an oracle with explicit aggregation configuration.
    pub fn with_config(sources: Vec<Box<dyn RateSource>>, config: AggregatorConfig) -> Self {
        Self {
            sources,
            config,
            cache: tokio::sync::Mutex::new(HashMap::new()),
            backoff_state: tokio::sync::Mutex::new(HashMap::new()),
            refresh: tokio::sync::Mutex::new(()),
        }
    }

    /// Return the configured source count.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Return backoff state for a source and unit, if one has been recorded.
    pub async fn backoff_state(
        &self,
        source_name: &str,
        fiat: &CurrencyUnit,
    ) -> Option<BackoffState> {
        self.backoff_state
            .lock()
            .await
            .get(&(source_name.to_owned(), fiat.clone()))
            .cloned()
    }
}

#[async_trait]
impl RateOracle for AggregatingRateOracle {
    async fn snapshot(&self, fiat: &CurrencyUnit) -> Result<RateSnapshot, RateOracleError> {
        if let Some(snapshot) = self.cached_snapshot(fiat).await {
            return Ok(snapshot);
        }

        // Single-flight: concurrent callers queue here; losers re-check the
        // cache and reuse the winner's snapshot instead of re-fetching.
        let _refresh = self.refresh.lock().await;
        if let Some(snapshot) = self.cached_snapshot(fiat).await {
            return Ok(snapshot);
        }

        let snapshot = self.fetch_snapshot(fiat).await?;
        self.cache.lock().await.insert(
            fiat.clone(),
            CachedSnapshot {
                snapshot: snapshot.clone(),
                cached_at: Instant::now(),
            },
        );
        Ok(snapshot)
    }
}

impl AggregatingRateOracle {
    async fn cached_snapshot(&self, fiat: &CurrencyUnit) -> Option<RateSnapshot> {
        let cache = self.cache.lock().await;
        let cached = cache.get(fiat)?;
        (cached.cached_at.elapsed() < Duration::from_secs(self.config.cache_ttl_secs))
            .then(|| cached.snapshot.clone())
    }

    async fn fetch_snapshot(&self, fiat: &CurrencyUnit) -> Result<RateSnapshot, RateOracleError> {
        let timeout = Duration::from_secs(self.config.fetch_timeout_secs);
        let fetch_started_at = Instant::now();

        let fetches = self.sources.iter().map(|source| async move {
            let source_name = source.name().to_owned();
            let local_fetch_started_at = SystemTime::now();
            let result = time::timeout(timeout, source.fetch(fiat)).await;
            match result {
                Ok(Ok((rate, source_reported_timestamp))) if rate > 0 => Ok(SourceReading {
                    source_name,
                    rate,
                    fetched_at_age_secs: fetch_started_at.elapsed().as_secs(),
                    source_reported_timestamp,
                    included_in_aggregation: false,
                }),
                Ok(Ok((rate, _))) => Err(FetchFailure {
                    source_name,
                    reason: format!("zero rate: {rate}"),
                    timed_out: false,
                    local_fetch_started_at,
                }),
                Ok(Err(error)) => Err(FetchFailure {
                    source_name,
                    reason: error.to_string(),
                    timed_out: false,
                    local_fetch_started_at,
                }),
                Err(_) => Err(FetchFailure {
                    source_name,
                    reason: "timeout".to_owned(),
                    timed_out: true,
                    local_fetch_started_at,
                }),
            }
        });

        let mut readings = Vec::new();
        let mut failures = Vec::new();
        for outcome in join_all(fetches).await {
            match outcome {
                Ok(reading) => readings.push(reading),
                Err(failure) => failures.push(failure),
            }
        }

        for failure in &failures {
            let _ = failure.local_fetch_started_at;
            self.record_failure(&failure.source_name, fiat).await;
        }

        if readings.is_empty() {
            if failures.iter().any(|failure| failure.timed_out) {
                return Err(RateOracleError::AllStale);
            }
            let message = failures
                .into_iter()
                .map(|failure| format!("{}: {}", failure.source_name, failure.reason))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(RateOracleError::SourceError(message));
        }

        if readings.len() < self.config.min_sources {
            return Err(RateOracleError::InsufficientSources {
                fetched: readings.len(),
                required: self.config.min_sources,
            });
        }

        let local_now = SystemTime::now();
        let mut fresh_readings = Vec::with_capacity(readings.len());
        for reading in readings {
            if self.is_stale(&reading, local_now) {
                self.record_failure(&reading.source_name, fiat).await;
            } else {
                self.clear_failure(&reading.source_name, fiat).await;
                fresh_readings.push(reading);
            }
        }

        if fresh_readings.is_empty() {
            return Err(RateOracleError::AllStale);
        }

        if fresh_readings.len() < self.config.min_sources {
            return Err(RateOracleError::InsufficientSources {
                fetched: fresh_readings.len(),
                required: self.config.min_sources,
            });
        }

        let median_before_trim =
            median(fresh_readings.iter().map(|reading| reading.rate).collect());
        let mut max_deviation_bps = 0_u64;
        let mut survived = Vec::new();
        let mut trimmed = Vec::new();

        for mut reading in fresh_readings {
            let deviation_bps = bps_deviation(reading.rate, median_before_trim);
            max_deviation_bps = max_deviation_bps.max(deviation_bps);
            if deviation_bps > self.config.deviation_threshold_bps {
                trimmed.push(reading);
            } else {
                reading.included_in_aggregation = true;
                survived.push(reading);
            }
        }

        if survived.len() < self.config.min_survived {
            return Err(RateOracleError::Divergence { max_deviation_bps });
        }

        // Trimmed median of the survivors. The even-count rule is the
        // arithmetic mean of the two middle values rounded DOWN: floor is the
        // explicit mint-favoring direction on the melt path (a lower
        // sats-per-fiat rate makes the user surrender more fiat per sat), and
        // for the 3-fetched/1-trimmed case it equals the
        // mean-of-the-surviving-2 rule. The mint-quote path's residual
        // half-step exposure is bounded by `deviation_threshold_bps` and
        // covered by the quote buffer.
        let aggregated_rate = median(survived.iter().map(|reading| reading.rate).collect());
        let sources_fetched = survived.len() + trimmed.len();
        let sources_trimmed = trimmed.len();
        let sources_survived = survived.len();
        let mut source_readings = survived;
        source_readings.extend(trimmed);

        Ok(RateSnapshot {
            fiat: fiat.clone(),
            aggregated_rate,
            source_readings,
            aggregation_meta: AggregationMeta {
                sources_fetched,
                sources_trimmed,
                sources_survived,
                median_before_trim,
                deviation_threshold_bps: self.config.deviation_threshold_bps,
            },
            created_at: SystemTime::now(),
        })
    }

    fn is_stale(&self, reading: &SourceReading, local_now: SystemTime) -> bool {
        if reading.fetched_at_age_secs > self.config.max_clock_offset_secs {
            return true;
        }

        reading.source_reported_timestamp.is_some_and(|timestamp| {
            system_time_abs_diff_secs(timestamp, local_now) > self.config.max_clock_offset_secs
        })
    }

    async fn record_failure(&self, source_name: &str, fiat: &CurrencyUnit) {
        let mut backoff_state = self.backoff_state.lock().await;
        let state = backoff_state
            .entry((source_name.to_owned(), fiat.clone()))
            .or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        state.last_failure_at = Some(Instant::now());
    }

    async fn clear_failure(&self, source_name: &str, fiat: &CurrencyUnit) {
        self.backoff_state
            .lock()
            .await
            .remove(&(source_name.to_owned(), fiat.clone()));
    }
}

#[derive(Debug)]
struct FetchFailure {
    source_name: String,
    reason: String,
    timed_out: bool,
    local_fetch_started_at: SystemTime,
}

/// Integer median. Even-count rule: floor of the mean of the two middle
/// values (see the aggregation comment for the mint-favoring rationale).
fn median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        let sum = values[mid - 1] as u128 + values[mid] as u128;
        u64::try_from(sum / 2).unwrap_or(u64::MAX)
    } else {
        values[mid]
    }
}

fn bps_deviation(value: u64, median: u64) -> u64 {
    if median == 0 {
        return u64::MAX;
    }
    let delta = value.abs_diff(median) as u128;
    let bps = delta.saturating_mul(10_000) / median as u128;
    u64::try_from(bps).unwrap_or(u64::MAX)
}

fn system_time_abs_diff_secs(left: SystemTime, right: SystemTime) -> u64 {
    left.duration_since(right)
        .or_else(|_| right.duration_since(left))
        .map_or(u64::MAX, |duration| duration.as_secs())
}
