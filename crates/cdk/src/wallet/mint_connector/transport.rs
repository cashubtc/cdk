//! Transport re-exports for wallet mint connector.

// The rate limiter lives in cdk-common so the logic is shared; re-export it here
// to keep the wallet's transport path stable.
pub use cdk_common::rate_limit::{
    RateLimitConfig, RateLimitedTransport, RateLimiterManager, TokenBucket,
};
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

    use super::{RateLimitConfig, RateLimiterManager, TokenBucket};
    use crate::cdk_database::WalletDatabase;
    use crate::mint_url::MintUrl;

    fn config(capacity: u32, refill_per_minute: u32) -> RateLimitConfig {
        RateLimitConfig::new(
            NonZeroU32::new(capacity).unwrap_or(NonZeroU32::MIN),
            NonZeroU32::new(refill_per_minute).unwrap_or(NonZeroU32::MIN),
        )
    }

    #[tokio::test]
    async fn manager_shares_one_bucket_per_origin() {
        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        // capacity 2 so two try_acquires exhaust one origin's shared burst.
        let manager = RateLimiterManager::new(config(2, 300), store);

        // Two URLs of the same origin (default port collapses to no port) must
        // resolve to one shared bucket.
        let implicit = MintUrl::from_str("https://mint.example.com").unwrap();
        let explicit = MintUrl::from_str("https://mint.example.com:443").unwrap();
        let a = manager.bucket_for(&implicit);
        let b = manager.bucket_for(&explicit);

        assert!(a.try_acquire().await);
        assert!(b.try_acquire().await);
        // The shared burst of 2 is now spent across both handles.
        assert!(!a.try_acquire().await, "shared origin budget is drained");
        assert!(!b.try_acquire().await, "shared origin budget is drained");

        // A different origin keeps its own budget.
        let other = manager.bucket_for(&MintUrl::from_str("https://other.example.com").unwrap());
        assert!(other.try_acquire().await, "distinct origin is independent");
    }

    #[tokio::test]
    async fn manager_evicts_recovered_unreferenced_bucket() {
        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let manager = RateLimiterManager::new(config(5, 300), store);

        // A fresh bucket is fully recovered. Dropping the handle leaves the map
        // as its only holder.
        let a = MintUrl::from_str("https://a.example.com").unwrap();
        drop(manager.bucket_for(&a));
        assert_eq!(manager.origin_count(), 1);

        // Creating a bucket for a different origin sweeps the recovered,
        // unreferenced A out, so only B remains.
        let b = MintUrl::from_str("https://b.example.com").unwrap();
        let _b = manager.bucket_for(&b);
        assert_eq!(manager.origin_count(), 1);
    }

    #[tokio::test]
    async fn manager_retains_referenced_bucket() {
        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let manager = RateLimiterManager::new(config(5, 300), store);

        // Hold a live clone of A: even though A is fully recovered, a live
        // holder must keep it so co-active wallets never split budgets.
        let a = MintUrl::from_str("https://a.example.com").unwrap();
        let _held = manager.bucket_for(&a);

        let b = MintUrl::from_str("https://b.example.com").unwrap();
        let _b = manager.bucket_for(&b);
        assert_eq!(manager.origin_count(), 2);
    }

    #[tokio::test]
    async fn manager_retains_unrecovered_bucket() {
        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let manager = RateLimiterManager::new(config(5, 300), store);

        // Drain A past its burst so it carries debt, then drop the handle: it is
        // unreferenced but not yet recovered, so it must be kept.
        let a = MintUrl::from_str("https://a.example.com").unwrap();
        let bucket_a = manager.bucket_for(&a);
        for _ in 0..5 {
            bucket_a.try_acquire().await;
        }
        drop(bucket_a);

        let b = MintUrl::from_str("https://b.example.com").unwrap();
        let _b = manager.bucket_for(&b);
        assert_eq!(manager.origin_count(), 2);
    }

    #[tokio::test]
    async fn shared_bucket_survives_concurrent_writers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store: Arc<dyn WalletDatabase<database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let url = MintUrl::from_str("https://concurrent.example.com").unwrap();
        // capacity 5, emission ~200ms so the inherited budget paces measurably.
        let cfg = config(5, 300);
        let manager = RateLimiterManager::new(cfg, store.clone());

        // Two handles to the same origin, as two currency-unit wallets hold.
        let a = manager.bucket_for(&url);
        let b = manager.bucket_for(&url);

        // Hammer both handles concurrently. They share one budget and one
        // writer, so the combined burst is exactly `capacity`, never twice it,
        // no matter how the four tasks interleave.
        let admitted = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for bucket in [a.clone(), b.clone(), a.clone(), b.clone()] {
            let admitted = admitted.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    if bucket.try_acquire().await {
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(
            admitted.load(Ordering::SeqCst),
            5,
            "concurrent handles share one burst, not one each"
        );

        // The single writer persists the drained budget; a fresh instance over
        // the same store inherits it (does not reset to full), proving no
        // concurrent writer overwrote it back to a staler value.
        a.flush().await;
        drop((a, b, manager));

        let fresh = TokenBucket::for_mint(cfg, &url, store.clone());
        let start = Instant::now();
        fresh.acquire(async {}).await;
        fresh.acquire(async {}).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "inherited budget should pace, took {:?}",
            start.elapsed()
        );
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
