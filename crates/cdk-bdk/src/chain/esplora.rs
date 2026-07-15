use std::time::Instant;

use bdk_esplora::esplora_client::{AsyncClient, Builder};
use bdk_esplora::EsploraAsyncExt;
use bdk_wallet::bitcoin::Transaction;
use cdk_common::redact::url_for_logs;
use tokio::time::{interval, Duration};
use tokio_util::sync::CancellationToken;

use crate::chain::{BroadcastErrorKind, BroadcastFailure, BroadcastOutcome, EsploraConfig};
use crate::error::Error;
use crate::{BdkWalletEngine, CdkBdk};

const MIN_ESPLORA_BACKOFF: Duration = Duration::from_secs(5);
const MAX_ESPLORA_BACKOFF: Duration = Duration::from_secs(300);

fn build_esplora_client(config: &EsploraConfig) -> Result<AsyncClient, Error> {
    config.validate()?;
    Builder::new(&config.url)
        .timeout(config.request_timeout_secs)
        .max_retries(config.max_retries)
        .build_async()
        .map_err(|err| Error::Esplora(err.to_string()))
}

fn next_esplora_backoff(backoff: &mut Duration) -> Duration {
    let current = *backoff;
    *backoff = (*backoff * 2).min(MAX_ESPLORA_BACKOFF);
    current
}

pub(crate) async fn sync_esplora_once(
    engine: &BdkWalletEngine,
    config: &EsploraConfig,
) -> Result<(), Error> {
    let client = build_esplora_client(config)?;
    let request = {
        let wallet_with_db = engine.wallet_with_db.lock().await;
        wallet_with_db.wallet.start_sync_with_revealed_spks()
    };
    let update = client
        .sync(request, config.parallel_requests)
        .await
        .map_err(|err| Error::Esplora(err.to_string()))?;
    let mut wallet_with_db = engine.wallet_with_db.lock().await;
    wallet_with_db
        .wallet
        .apply_update_events(update)
        .map_err(|err| Error::Wallet(err.to_string()))?;
    wallet_with_db.persist()?;
    Ok(())
}

pub(crate) async fn sync_esplora(
    cdk_bdk: &CdkBdk,
    config: &EsploraConfig,
    cancel_token: CancellationToken,
) -> Result<(), Error> {
    let parallel_requests = config.parallel_requests;
    let configured_interval = Duration::from_secs(cdk_bdk.sync_interval_secs);
    let initial_backoff = configured_interval.max(MIN_ESPLORA_BACKOFF);
    let mut sync_interval = interval(configured_interval);
    let warn_ms = cdk_bdk.sync_config.lock_hold_warn_ms;

    // Persist Esplora client across sync iterations; re-create on error.
    let mut esplora_client: Option<AsyncClient> = None;
    let mut consecutive_failures: u32 = 0;
    let mut backoff = initial_backoff;

    tracing::info!(
        url = %url_for_logs(url),
        parallel_requests,
        interval_secs = cdk_bdk.sync_interval_secs,
        "Starting Esplora block sync"
    );
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("Wallet sync cancelled via cancel token");
                break;
            }
            _ = sync_interval.tick() => {
                let client = match &esplora_client {
                    Some(c) => c.clone(),
                    None => {
                        match build_esplora_client(config) {
                            Ok(c) => {
                                esplora_client = Some(c.clone());
                                c
                            }
                            Err(e) => {
                                consecutive_failures =
                                    consecutive_failures.saturating_add(1);
                                let err = Error::Esplora(e.to_string());
                                crate::sync::log_sync_failure(
                                    "Failed to construct Esplora client",
                                    &err,
                                    consecutive_failures,
                                );
                                let retry_delay = next_esplora_backoff(&mut backoff);
                                tracing::warn!(
                                    retry_delay_secs = retry_delay.as_secs(),
                                    "Backing off Esplora sync retry"
                                );
                                sync_interval.reset_after(retry_delay);
                                continue;
                            }
                        }
                    }
                };

                // Phase A (short lock): build the sync request.
                let sync_request = {
                    let w = cdk_bdk.wallet_engine.wallet_with_db.lock().await;
                    w.wallet.start_sync_with_revealed_spks()
                };

                // Phase B (no lock): execute the network sync.
                let sync_update = match client.sync(sync_request, parallel_requests).await {
                    Ok(u) => u,
                    Err(e) => {
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        let err = Error::Esplora(e.to_string());
                        crate::sync::log_sync_failure(
                            "Esplora sync failed",
                            &err,
                            consecutive_failures,
                        );
                        // Drop client so the next tick rebuilds it.
                        esplora_client = None;
                        let retry_delay = next_esplora_backoff(&mut backoff);
                        tracing::warn!(
                            retry_delay_secs = retry_delay.as_secs(),
                            "Backing off Esplora sync retry"
                        );
                        sync_interval.reset_after(retry_delay);
                        continue;
                    }
                };

                // Phase C (short lock): apply the update and persist.
                let apply_result = {
                    let apply_start = Instant::now();
                    let mut w = cdk_bdk.wallet_engine.wallet_with_db.lock().await;
                    let res = w
                        .wallet
                        .apply_update_events(sync_update)
                        .map_err(|e| Error::Wallet(e.to_string()))
                        .and_then(|events| {
                            w.persist()?;
                            Ok(events)
                        });
                    let elapsed_ms = apply_start.elapsed().as_millis() as u64;
                    if elapsed_ms > warn_ms {
                        tracing::warn!(
                            held_ms = elapsed_ms,
                            warn_ms,
                            "Wallet lock held longer than configured warning threshold during esplora apply"
                        );
                    }
                    res
                };

                if let Err(e) = apply_result {
                        consecutive_failures =
                            consecutive_failures.saturating_add(1);
                        crate::sync::log_sync_failure(
                            "Failed to apply Esplora update",
                            &e,
                            consecutive_failures,
                        );
                        continue;
                }

                let tip = {
                    let w = cdk_bdk.wallet_engine.wallet_with_db.lock().await;
                    w.wallet.latest_checkpoint().block_id()
                };
                tracing::info!(
                    "Esplora synced to block {} at height {}",
                    tip.hash,
                    tip.height
                );

                if consecutive_failures > 0 {
                    tracing::info!(
                        recovered_after = consecutive_failures,
                        "Esplora sync recovered"
                    );
                    consecutive_failures = 0;
                }
                backoff = initial_backoff;

                cdk_bdk.run_reconciliation().await;
            }
        }
    }
    Ok(())
}

pub(crate) fn classify_esplora_broadcast_error(message: &str) -> BroadcastErrorKind {
    let lower = message.to_ascii_lowercase();

    if lower.contains("already")
        && (lower.contains("known") || lower.contains("mempool") || lower.contains("chain"))
    {
        return BroadcastErrorKind::Unknown;
    }

    if lower.contains("dust")
        || lower.contains("min relay")
        || lower.contains("minrelay")
        || lower.contains("mandatory-script-verify-flag-failed")
        || lower.contains("non-mandatory-script-verify-flag")
        || lower.contains("bad-txns")
        || lower.contains("nonstandard")
        || lower.contains("non-standard")
        || lower.contains("insufficient fee")
        || lower.contains("fee too low")
        || lower.contains("mempool min fee")
        || lower.contains("missing inputs")
        || lower.contains("txn-mempool-conflict")
        || lower.contains("replacement-adds-unconfirmed")
    {
        return BroadcastErrorKind::Rejected;
    }

    if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("connect")
        || lower.contains("dns")
        || lower.contains("http")
        || lower.contains("status 5")
        || lower.contains("temporarily unavailable")
        || lower.contains("too many requests")
    {
        return BroadcastErrorKind::Transient;
    }

    BroadcastErrorKind::Unknown
}

pub(crate) async fn broadcast_esplora(
    config: &EsploraConfig,
    tx: Transaction,
) -> Result<BroadcastOutcome, BroadcastFailure> {
    let client = build_esplora_client(config)
        .map_err(|e| BroadcastFailure::new(BroadcastErrorKind::Transient, e.to_string()))?;

    tracing::info!(
        "Broadcasting transaction: {} via esplora",
        tx.compute_txid()
    );

    match client.broadcast(&tx).await {
        Ok(()) => Ok(BroadcastOutcome::Accepted),
        Err(e) => {
            let message = e.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("already")
                && (lower.contains("known") || lower.contains("mempool") || lower.contains("chain"))
            {
                return Ok(BroadcastOutcome::AlreadyKnown);
            }

            Err(BroadcastFailure::new(
                classify_esplora_broadcast_error(&message),
                message,
            ))
        }
    }
}

pub(crate) async fn fetch_fee_rate_esplora(
    config: &EsploraConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    let client = build_esplora_client(config)?;

    let estimates = client
        .get_fee_estimates()
        .await
        .map_err(|e| Error::Esplora(e.to_string()))?;

    // Esplora returns a map of target blocks (as u16) to fee rate (sat/vB as f64)
    if let Some(&rate) = estimates.get(&target_blocks) {
        return Ok(rate);
    }

    // Fallback: find the closest available target block estimate that is >= our target
    let mut available_targets: Vec<u16> = estimates.keys().copied().collect();
    available_targets.sort_unstable();

    for &t in &available_targets {
        if t >= target_blocks {
            if let Some(&rate) = estimates.get(&t) {
                return Ok(rate);
            }
        }
    }

    // If nothing >= target, take the largest available
    if let Some(&t) = available_targets.last() {
        if let Some(&rate) = estimates.get(&t) {
            return Ok(rate);
        }
    }

    Err(Error::FeeEstimationUnavailable)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    use bdk_wallet::bitcoin::bip32::{Xpriv, Xpub};
    use bdk_wallet::bitcoin::secp256k1::Secp256k1;
    use bdk_wallet::bitcoin::{absolute, transaction, Network};

    use super::*;

    struct MockResponse {
        status: u16,
        body: &'static str,
        delay: Duration,
    }

    fn start_mock_esplora(
        responses: Vec<MockResponse>,
    ) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Esplora");
        let address = listener.local_addr().expect("mock Esplora address");
        let requests = Arc::new(AtomicUsize::new(0));
        let request_counter = Arc::clone(&requests);
        let server = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept mock request");
                request_counter.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 16 * 1024];
                let _ = stream.read(&mut request);
                thread::sleep(response.delay);
                let reason = match response.status {
                    200 => "OK",
                    400 => "Bad Request",
                    429 => "Too Many Requests",
                    _ => "Service Unavailable",
                };
                let reply = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    reason,
                    response.body.len(),
                    response.body
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (format!("http://{address}"), requests, server)
    }

    fn config(url: String, timeout: u64, retries: usize) -> EsploraConfig {
        EsploraConfig {
            url,
            parallel_requests: 1,
            request_timeout_secs: timeout,
            max_retries: retries,
        }
    }

    fn empty_transaction() -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        }
    }

    #[test]
    fn classify_esplora_broadcast_errors() {
        assert_eq!(
            classify_esplora_broadcast_error("sendrawtransaction RPC error: missing inputs"),
            BroadcastErrorKind::Rejected
        );
        assert_eq!(
            classify_esplora_broadcast_error("connection timeout"),
            BroadcastErrorKind::Transient
        );
        assert_eq!(
            classify_esplora_broadcast_error("unexpected backend response"),
            BroadcastErrorKind::Unknown
        );
    }

    #[tokio::test]
    async fn fee_estimate_uses_compatible_esplora_response_and_nearest_target() {
        let (url, requests, server) = start_mock_esplora(vec![MockResponse {
            status: 200,
            body: r#"{"1":12.5,"6":4.0,"144":1.2}"#,
            delay: Duration::ZERO,
        }]);

        let rate = fetch_fee_rate_esplora(&config(url, 2, 0), 3)
            .await
            .expect("fee estimate");

        assert_eq!(rate, 4.0);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.join().expect("mock Esplora server");
    }

    #[tokio::test]
    async fn broadcast_accepts_success_and_already_known() {
        let (url, _, server) = start_mock_esplora(vec![MockResponse {
            status: 200,
            body: "accepted",
            delay: Duration::ZERO,
        }]);
        assert_eq!(
            broadcast_esplora(&config(url, 2, 0), empty_transaction())
                .await
                .expect("accepted broadcast"),
            BroadcastOutcome::Accepted
        );
        server.join().expect("accepted server");

        let (url, _, server) = start_mock_esplora(vec![MockResponse {
            status: 400,
            body: "transaction already in mempool",
            delay: Duration::ZERO,
        }]);
        assert_eq!(
            broadcast_esplora(&config(url, 2, 0), empty_transaction())
                .await
                .expect("already-known broadcast"),
            BroadcastOutcome::AlreadyKnown
        );
        server.join().expect("already-known server");
    }

    #[tokio::test]
    async fn rate_limit_is_retried_within_configured_bound() {
        let (url, requests, server) = start_mock_esplora(vec![
            MockResponse {
                status: 429,
                body: "rate limited",
                delay: Duration::ZERO,
            },
            MockResponse {
                status: 200,
                body: r#"{"1":8.0}"#,
                delay: Duration::ZERO,
            },
        ]);

        assert_eq!(
            fetch_fee_rate_esplora(&config(url, 2, 1), 1)
                .await
                .expect("retry succeeds"),
            8.0
        );
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.join().expect("retry server");
    }

    #[tokio::test]
    async fn malformed_and_timed_out_responses_fail_closed() {
        let (url, _, server) = start_mock_esplora(vec![MockResponse {
            status: 200,
            body: "not-json",
            delay: Duration::ZERO,
        }]);
        assert!(fetch_fee_rate_esplora(&config(url, 2, 0), 1).await.is_err());
        server.join().expect("malformed server");

        let (url, _, server) = start_mock_esplora(vec![MockResponse {
            status: 200,
            body: r#"{"1":8.0}"#,
            delay: Duration::from_secs(2),
        }]);
        assert!(fetch_fee_rate_esplora(&config(url, 1, 0), 1).await.is_err());
        server.join().expect("timeout server");
    }

    #[tokio::test]
    async fn sync_network_wait_does_not_hold_the_wallet_lock() {
        let (url, requests, server) = start_mock_esplora(vec![MockResponse {
            status: 200,
            body: "not-a-tip-height",
            delay: Duration::from_millis(500),
        }]);
        let secp = Secp256k1::new();
        let xpriv = Xpriv::new_master(Network::Regtest, &[41; 32]).expect("master key");
        let xpub = Xpub::from_priv(&secp, &xpriv);
        let descriptors = crate::DescriptorConfig::new(
            format!("tr({xpub}/0/*)"),
            format!("tr({xpub}/1/*)"),
            Network::Regtest,
        )
        .expect("descriptors");
        let root = tempfile::tempdir().expect("wallet directory");
        let config = config(url, 2, 0);
        let engine = BdkWalletEngine::open_watch_only(
            descriptors,
            crate::ChainSource::Esplora(config.clone()),
            root.path(),
        )
        .expect("watch-only wallet");
        engine
            .reveal_addresses_to(bdk_wallet::KeychainKind::External, 0)
            .await
            .expect("reveal one script");
        let sync_engine = engine.clone();
        let sync = tokio::spawn(async move { sync_esplora_once(&sync_engine, &config).await });

        tokio::time::timeout(Duration::from_secs(1), async {
            while requests.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("network request started");
        let wallet = tokio::time::timeout(Duration::from_millis(100), engine.wallet_with_db.lock())
            .await
            .expect("wallet lock remains available during network request");
        drop(wallet);

        assert!(sync.await.expect("sync task").is_err());
        server.join().expect("delayed sync server");
    }
}
