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
    Rate(u64),
    RateWithTimestamp(u64, SystemTime),
    Error(String),
    DelayThenRate(Duration, u64),
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
    ) -> Result<(u64, Option<SystemTime>), RateOracleError> {
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

fn source(name: &'static str, rate: u64) -> Box<dyn RateSource> {
    CannedSource::boxed(name, Behavior::Rate(rate)).0
}

fn oracle(sources: Vec<Box<dyn RateSource>>) -> AggregatingRateOracle {
    AggregatingRateOracle::new(sources)
}

#[tokio::test]
async fn test_happy_path_three_sources() {
    let oracle = oracle(vec![source("a", 1000), source("b", 1001), source("c", 999)]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(snapshot.aggregated_rate, 1000);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 0);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 3);
}

#[tokio::test]
async fn test_one_source_skewed_trimmed() {
    let oracle = oracle(vec![
        source("a", 1000),
        source("b", 1002),
        source("c", 2000),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    // Survivors are 1000 and 1002: even-count rule is the floor of the mean
    // of the two middle values.
    assert_eq!(snapshot.aggregated_rate, 1001);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 1);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 2);
}

#[tokio::test]
async fn test_exactly_three_fetched_one_trimmed_uses_floor_mean_of_two() {
    let oracle = oracle(vec![source("a", 100), source("b", 101), source("c", 200)]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    // ADR-023 trimming rule: 3 fetched, 1 trimmed → mean of the surviving 2,
    // floored (mint-favoring on the melt path): floor((100 + 101) / 2) = 100.
    assert_eq!(snapshot.aggregated_rate, 100);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.aggregation_meta.sources_trimmed, 1);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 2);
}

#[tokio::test]
async fn test_four_survivors_even_count_floor_mean_of_middles() {
    let oracle = oracle(vec![
        source("a", 1000),
        source("b", 1001),
        source("c", 1003),
        source("d", 1004),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(snapshot.aggregated_rate, 1002);
    assert_eq!(snapshot.aggregation_meta.sources_survived, 4);
}

#[tokio::test]
async fn test_insufficient_sources_fail_closed() {
    let oracle = oracle(vec![source("a", 1000), source("b", 1001)]);

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
        CannedSource::boxed("a", Behavior::RateWithTimestamp(1000, stale_time)).0,
        CannedSource::boxed("b", Behavior::RateWithTimestamp(1001, stale_time)).0,
        CannedSource::boxed("c", Behavior::RateWithTimestamp(999, stale_time)).0,
    ]);

    let error = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap_err();

    assert!(matches!(error, RateOracleError::AllStale));
}

#[tokio::test]
async fn test_divergence_fail_closed() {
    let oracle = oracle(vec![source("a", 10), source("b", 100), source("c", 1000)]);

    let error = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap_err();

    assert!(matches!(error, RateOracleError::Divergence { .. }));
}

#[tokio::test]
async fn test_cache_hit() {
    let (a, a_calls) = CannedSource::boxed("a", Behavior::Rate(1000));
    let (b, b_calls) = CannedSource::boxed("b", Behavior::Rate(1001));
    let (c, c_calls) = CannedSource::boxed("c", Behavior::Rate(999));
    let oracle = oracle(vec![a, b, c]);

    let first = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();
    let second = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(first.aggregated_rate, second.aggregated_rate);
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
    assert_eq!(c_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_cache_expiry() {
    let (a, a_calls) = CannedSource::boxed("a", Behavior::Rate(1000));
    let (b, b_calls) = CannedSource::boxed("b", Behavior::Rate(1001));
    let (c, c_calls) = CannedSource::boxed("c", Behavior::Rate(999));
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
        CannedSource::boxed("a", Behavior::RateWithTimestamp(5000, invalid_time)).0,
        source("b", 100),
        source("c", 101),
        source("d", 99),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(snapshot.aggregated_rate, 100);
    assert_eq!(snapshot.aggregation_meta.sources_fetched, 3);
    assert_eq!(snapshot.source_readings.len(), 3);
}

#[tokio::test]
async fn test_concurrent_fetch_timeout_isolation() {
    let (slow, slow_calls) = CannedSource::boxed(
        "slow",
        Behavior::DelayThenRate(Duration::from_secs(2), 5000),
    );
    let config = AggregatorConfig {
        fetch_timeout_secs: 1,
        ..AggregatorConfig::default()
    };
    let oracle = AggregatingRateOracle::with_config(
        vec![slow, source("a", 100), source("b", 100), source("c", 100)],
        config,
    );
    let started_at = std::time::Instant::now();

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert!(started_at.elapsed() < Duration::from_millis(1500));
    assert_eq!(snapshot.aggregated_rate, 100);
    assert_eq!(slow_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_concurrent_snapshot_requests_single_flight() {
    let delay = Duration::from_millis(100);
    let (a, a_calls) = CannedSource::boxed("a", Behavior::DelayThenRate(delay, 1000));
    let (b, b_calls) = CannedSource::boxed("b", Behavior::DelayThenRate(delay, 1001));
    let (c, c_calls) = CannedSource::boxed("c", Behavior::DelayThenRate(delay, 999));
    let oracle = Arc::new(oracle(vec![a, b, c]));

    let (first, second) = tokio::join!(
        {
            let oracle = oracle.clone();
            async move { oracle.snapshot(&CurrencyUnit::Usd).await }
        },
        {
            let oracle = oracle.clone();
            async move { oracle.snapshot(&CurrencyUnit::Usd).await }
        }
    );

    assert_eq!(
        first.unwrap().aggregated_rate,
        second.unwrap().aggregated_rate
    );
    assert_eq!(a_calls.load(Ordering::SeqCst), 1);
    assert_eq!(b_calls.load(Ordering::SeqCst), 1);
    assert_eq!(c_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_backoff_state_per_source_independent() {
    let oracle = oracle(vec![
        CannedSource::boxed("a", Behavior::Error("boom".to_owned())).0,
        source("b", 100),
        source("c", 101),
        source("d", 99),
    ]);

    let snapshot = oracle.snapshot(&CurrencyUnit::Usd).await.unwrap();

    assert_eq!(snapshot.aggregated_rate, 100);
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
