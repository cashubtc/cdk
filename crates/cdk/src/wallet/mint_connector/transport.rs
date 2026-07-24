//! Transport re-exports for wallet mint connector.

// The rate limiter lives in cdk-common so the logic is shared; re-export it here
// to keep the wallet's transport path stable.
pub use cdk_common::rate_limit::{RateLimitConfig, RateLimitedTransport, TokenBucket};
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use cdk_http_client::TorAsync;
pub use cdk_http_client::{Async, Transport};

// The persistence round-trip test lives here rather than in cdk-common: it needs
// a concrete `WalletDatabase`, and the in-memory one is in `cdk-sqlite`, which
// depends on `cdk-common` (so testing it there would be a dependency cycle).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod persistence_tests {
    use std::num::NonZeroU32;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cdk_common::database;

    use super::{RateLimitConfig, TokenBucket};
    use crate::cdk_database::WalletDatabase;
    use crate::mint_url::MintUrl;

    fn config(capacity: u32, refill_per_minute: u32) -> RateLimitConfig {
        RateLimitConfig::new(
            NonZeroU32::new(capacity).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(refill_per_minute).unwrap_or(NonZeroU32::MIN),
        )
    }

    #[tokio::test]
    async fn budget_persists_across_instances() {
        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let url = MintUrl::from_str("https://persist.example.com").unwrap();
        // capacity 2, emission ~200ms so the pace signal clears scheduler noise.
        let cfg = config(2, 300);

        // Drain the first bucket's burst, flush it to the store, then drop it.
        let first = TokenBucket::for_mint(cfg, &url, store.clone());
        first.acquire(async {}).await;
        first.acquire(async {}).await;
        first.flush().await;
        drop(first);

        // A second bucket for the same host inherits the drained budget: it is
        // at the burst edge, so two acquires take about one emission interval.
        let second = TokenBucket::for_mint(cfg, &url, store.clone());
        let start = Instant::now();
        second.acquire(async {}).await;
        second.acquire(async {}).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "inherited bucket should pace, took {:?}",
            start.elapsed()
        );

        // A fresh, non-persisted bucket bursts both immediately for contrast.
        let fresh = TokenBucket::new(cfg);
        let start = Instant::now();
        fresh.acquire(async {}).await;
        fresh.acquire(async {}).await;
        assert!(start.elapsed() < Duration::from_millis(100));
    }
}
