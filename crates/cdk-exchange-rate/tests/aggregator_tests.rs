//! Aggregating oracle behavior tests with canned, no-network sources.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use cdk_common::nuts::CurrencyUnit;
use cdk_exchange_rate::{
    AggregatingRateOracle, AggregatorConfig, RateOracle, RateOracleError, RateSource,
};
use tokio::time;

#[derive(Debug, Clone)]
enum Behavior {
    Rate(f64),
    RateWithTimestamp(f64, SystemTime),
    Error(String),
    DelayThenRate(Duration, f64),
}

#[derive(Debug)]
struct CannedSource {
    name: String,
    behavior: Behavior,
    calls: Arc<AtomicUsize>,
}

impl CannedSource {
    fn boxed(
        name: impl Into<String>,
        behavior: Behavior,
    ) -> (Box<dyn RateSource>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Box::new(Self {
                name: name.into(),
                behavior,
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

#[async_trait]
impl RateSource for CannedSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn fetch(
        &self,
        _fiat: &CurrencyUnit,
    ) -> Result<(f64, Option<SystemTime>), RateOracleError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            Behavior::Rate(rate) => Ok((*rate, None)),
            Behavior::RateWithTimestamp(rate, timestamp) => Ok((*rate, Some(*timestamp))),
            Behavior::Error(message) => Err(RateOracleError::SourceError(message.clone())),
            Behavior::DelayThenRate(duration, rate) => {
                time::sleep(*duration).await;
                Ok((*rate, None))
            }
        }
    }
}

fn source(name: &'static str, rate: f64) -> Box<dyn RateSource> {
    CannedSource::boxed(name, Behavior::Rate(rate)).0
}

fn oracle(sources: Vec<Box<dyn RateSource>>) -> AggregatingRateOracle {
    AggregatingRateOracle::new(sources)
}

fn assert_approx_eq(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < f64::EPSILON,
        "actual {actual}, expected {expected}"
    );
}

#[tokio::test]
async fn test_happy_path_three_sources() {
    let oracle = oracle(vec![
        source("a", 1000.0),
        source("b", 1001.0),
        source("c", 999.0),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(snapshot.aggregated_rate, 1000.0);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 0);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 3);
}

#[tokio::test]
async fn test_one_source_skewed_trimmed() {
    let oracle = oracle(vec![
        source("a", 1000.0),
        source("b", 1002.0),
        source("c", 2000.0),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(snapshot.aggregated_rate, 1001.0);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 1);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 2);
}

#[tokio::test]
async fn test_exactly_three_fetched_one_trimmed_mean_of_two() {
    let oracle = oracle(vec![
        source("a", 100.0),
        source("b", 101.0),
        source("c", 200.0),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(snapshot.aggregated_rate, 100.5);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 1);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 2);
}

#[tokio::test]
async fn test_insufficient_sources_fail_closed() {
    let oracle = oracle(vec![source("a", 1000.0), source("b", 1001.0)]);

    let error = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap_err();

    assert!(matches!(
        error,
        RateOracleError::InsufficientSources {
            fetched: 2,
            required: 3
        }
    ));
}

#[tokio::test]
async fn test_all_stale_fail_closed() {
    let stale_time = SystemTime::now() - Duration::from_secs(60);
    let oracle = oracle(vec![
        CannedSource::boxed("a", Behavior::RateWithTimestamp(1000.0, stale_time)).0,
        CannedSource::boxed("b", Behavior::RateWithTimestamp(1001.0, stale_time)).0,
        CannedSource::boxed("c", Behavior::RateWithTimestamp(999.0, stale_time)).0,
    ]);

    let error = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap_err();

    assert!(matches!(error, RateOracleError::AllStale));
}

#[tokio::test]
async fn test_divergence_fail_closed() {
    let oracle = oracle(vec![
        source("a", 10.0),
        source("b", 100.0),
        source("c", 1000.0),
    ]);

    let error = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap_err();

    assert!(matches!(error, RateOracleError::Divergence { .. }));
}

#[tokio::test]
async fn test_cache_hit() {
    let (a, a_calls) = CannedSource::boxed("a", Behavior::Rate(1000.0));
    let (b, b_calls) = CannedSource::boxed("b", Behavior::Rate(1001.0));
    let (c, c_calls) = CannedSource::boxed("c", Behavior::Rate(999.0));
    let oracle = oracle(vec![a, b, c]);

    let first = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();
    let second = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(first.aggregated_rate, second.aggregated_rate);
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
    assert_eq!(c_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_cache_expiry() {
    let (a, a_calls) = CannedSource::boxed("a", Behavior::Rate(1000.0));
    let (b, b_calls) = CannedSource::boxed("b", Behavior::Rate(1001.0));
    let (c, c_calls) = CannedSource::boxed("c", Behavior::Rate(999.0));
    let config = AggregatorConfig {
        cache_ttl_secs: 0,
        ..AggregatorConfig::default()
    };
    let oracle = AggregatingRateOracle::with_config(vec![a, b, c], config);

    let _ = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();
    let _ = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(a_calls.load(Ordering::SeqCst), 2);
    assert_eq!(b_calls.load(Ordering::SeqCst), 2);
    assert_eq!(c_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_clock_offset_rejection() {
    let invalid_time = SystemTime::now() + Duration::from_secs(60);
    let oracle = oracle(vec![
        CannedSource::boxed("a", Behavior::RateWithTimestamp(5000.0, invalid_time)).0,
        source("b", 100.0),
        source("c", 101.0),
        source("d", 99.0),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(snapshot.aggregated_rate, 100.0);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.source_readings.len(), 3);
}

#[tokio::test]
async fn test_concurrent_fetch_timeout_isolation() {
    let (slow, slow_calls) = CannedSource::boxed(
        "slow",
        Behavior::DelayThenRate(Duration::from_secs(2), 5000.0),
    );
    let config = AggregatorConfig {
        fetch_timeout_secs: 1,
        ..AggregatorConfig::default()
    };
    let oracle = AggregatingRateOracle::with_config(
        vec![
            slow,
            source("a", 100.0),
            source("b", 100.0),
            source("c", 100.0),
        ],
        config,
    );
    let started_at = std::time::Instant::now();

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert!(started_at.elapsed() < Duration::from_millis(1500));
    assert_approx_eq(snapshot.aggregated_rate, 100.0);
    assert_eq!(slow_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_backoff_state_per_source_independent() {
    let oracle = oracle(vec![
        CannedSource::boxed("a", Behavior::Error("boom".to_owned())).0,
        source("b", 100.0),
        source("c", 101.0),
        source("d", 99.0),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_approx_eq(snapshot.aggregated_rate, 100.0);
    assert_eq!(
        oracle
            .backoff_state("a", &CurrencyUnit::Usd)
            .await
            .unwrap()
            .consecutive_failures(),
        1
    );
    assert!(oracle
        .backoff_state("b", &CurrencyUnit::Usd)
        .await
        .is_none());
}
