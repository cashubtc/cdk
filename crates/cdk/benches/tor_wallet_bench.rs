// Benchmarks for Tor bootstrapping and subsequent wallet/client operations over Tor
// Run with: cargo bench -p cdk --features tor --bench tor_wallet_bench
// Note: These benches hit a fake mint URL and primarily measure HTTP timings (including failures).
// They are intended to quantify: (1) initial Tor bootstrap latency, (2) subsequent request latency

#![cfg(all(feature = "wallet", feature = "tor", not(target_arch = "wasm32")))]

use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use cdk::wallet::{TorHttpClient, WalletBuilder, MintConnector};
use cdk::{nuts::CurrencyUnit, wallet::Wallet};

use cdk_sqlite::wallet::memory;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;
use rand::random;
use tokio::runtime::Runtime;

const MINT_URL: &str = "https://fake.thesimplekid.dev";
// Example BOLT11 invoice string (from docs); used only to exercise the HTTP path.
const DUMMY_BOLT11: &str = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq";

fn build_rt() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio runtime")
}

async fn build_wallet_tor() -> Result<Wallet> {
    let seed = random::<[u8; 64]>();
    let unit = CurrencyUnit::Sat;

    let localstore = memory::empty().await?;

    let mint_url = cdk::mint_url::MintUrl::from_str(MINT_URL)?;

    let tor_client = TorHttpClient::new(mint_url.clone(), None);

    let wallet = WalletBuilder::new()
        .mint_url(mint_url)
        .unit(unit)
        .localstore(Arc::new(localstore))
        .seed(seed)
        .shared_client(Arc::new(tor_client))
        .build()?;

    Ok(wallet)
}

fn bench_tor_bootstrap_and_warm_requests(c: &mut Criterion) {
    let mut group = c.benchmark_group("tor_wallet_http");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(60));

    // Cold info (includes Tor bootstrap each iteration by constructing a fresh wallet)
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("cold_info", "GET /v1/info"), |b| {
        b.iter_custom(|iters| {
            let rt = build_rt();
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let res: Result<()> = rt.block_on(async move {
                    let wallet = build_wallet_tor().await?;
                    let _ = wallet.fetch_mint_info().await; // we only care about timing
                    Ok(())
                });
                let _ = black_box(res);
                total += start.elapsed();
            }
            total
        })
    });

    // Warm info: bootstrap once, then measure subsequent GET /v1/info
    group.bench_function(BenchmarkId::new("warm_info", "GET /v1/info"), |b| {
        let rt = build_rt();
        // Pre-bootstrap and prime the exact endpoint under test
        let wallet = rt.block_on(async { build_wallet_tor().await.unwrap() });
        // Prime: first call builds circuits for this endpoint; exclude it from timing
        let _ = rt.block_on(async { wallet.fetch_mint_info().await });

        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let res = rt.block_on(async { wallet.fetch_mint_info().await });
                let _ = black_box(&res); // may fail; we measure time-to-response
                total += start.elapsed();
            }
            total
        })
    });

    // Warm POST: mint quote
    group.bench_function(BenchmarkId::new("warm_mint_quote", "POST /v1/mint/quote/bolt11"), |b| {
        let rt = build_rt();
        // Pre-bootstrap and prime the exact endpoint under test
        let wallet = rt.block_on(async { build_wallet_tor().await.unwrap() });
        // Prime: first call builds circuits for this endpoint; exclude it from timing
        let _ = rt.block_on(async { wallet.fetch_mint_info().await });

        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                // Use a minimal amount; server may return error. That's fine for timing.
                let res = rt.block_on(async {
                    // Calling the client API directly avoids extra DB work in mint_quote
                    // but still goes through Tor. However, we stick to wallet.mint_quote as requested.
                    wallet.mint_quote(cdk::Amount::from(1u64), None).await
                });
                let _ = black_box(&res);
                total += start.elapsed();
            }
            total
        })
    });

    // Warm POST: melt quote (requires an invoice; use a dummy/small invoice)
    group.bench_function(BenchmarkId::new("warm_melt_quote", "POST /v1/melt/quote/bolt11"), |b| {
        let rt = build_rt();
        // Pre-bootstrap and prime the exact endpoint under test
        let wallet = rt.block_on(async { build_wallet_tor().await.unwrap() });
        // Prime: first call builds circuits for this endpoint; exclude it from timing
        let _ = rt.block_on(async { wallet.fetch_mint_info().await });

        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let res = rt.block_on(async {
                    wallet.melt_quote(DUMMY_BOLT11.to_string(), None).await
                });
                let _ = black_box(&res);
                total += start.elapsed();
            }
            total
        })
    });

    // Warm POST: swap - we hit the endpoint using the client type with an empty swap request.
    group.bench_function(BenchmarkId::new("warm_swap", "POST /v1/swap"), |b| {
        let rt = build_rt();
        // Build a raw Tor client to call swap directly (Wallet.swap requires real proofs)
        let tor_client = {
            let mint_url = cdk::mint_url::MintUrl::from_str(MINT_URL).unwrap();
            TorHttpClient::new(mint_url, None)
        };
        // Pre-bootstrap via a cheap GET
        let _ = rt.block_on(async {
            tor_client.get_mint_info().await
        });

        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let res = rt.block_on(async {
                    // An empty swap request will likely 4xx but exercises the endpoint over Tor
                    let req = cdk_common::nuts::SwapRequest::new(vec![], vec![]);
                    tor_client.post_swap(req).await
                });
                let _ = black_box(&res);
                total += start.elapsed();
            }
            total
        })
    });

    group.finish();
}

criterion_group!(benches, bench_tor_bootstrap_and_warm_requests);
criterion_main!(benches);
