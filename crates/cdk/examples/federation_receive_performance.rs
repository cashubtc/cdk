//! Persistent SDK receiver for tokens and externally funded mint quotes.
//!
//! Pass one private JSON manifest. This driver never funds, retries failed
//! payments, or replaces state. Keep its directory for recovery and audit.
#![allow(missing_docs)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::{KnownMethod, ProofsMethods};
use cdk::nuts::{CurrencyUnit, PaymentMethod, Token};
use cdk::wallet::{FederatedMintConnector, ReceiveOptions, Wallet, WalletBuilder};
use cdk_common::federation::FederationConfig;
use cdk_sqlite::WalletSqliteDatabase;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio::time::{sleep_until, timeout_at, Instant};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{0}")]
    Input(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Wallet(#[from] cdk::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Duration(#[from] std::num::TryFromIntError),
}

type Result<T> = std::result::Result<T, Error>;

fn check(condition: bool, reason: &'static str) -> Result<()> {
    if !condition {
        return Err(Error::Input(reason));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    public_config: PathBuf,
    #[serde(default)]
    token_files: Vec<PathBuf>,
    #[serde(default)]
    mint_requests: Vec<MintRequest>,
    state_dir: PathBuf,
    concurrency: usize,
    window_ms: u64,
    drain_ms: u64,
    /// None selects closed loop. Open loop has zero queued requests and rejects
    /// offered arrivals when all concurrency slots are occupied.
    arrival_interval_ms: Option<u64>,
    shared_wallet: bool,
    warm_connections: bool,
    #[serde(default)]
    trace_performance: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MintRequest {
    method: MintMethod,
    amount_sat: u64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum MintMethod {
    Bolt11,
    Onchain,
}

enum Input {
    Token(String),
    Mint(MintRequest),
}

#[derive(Serialize)]
struct Attempt {
    id: usize,
    scheduled_ns: u64,
    started_ns: Option<u64>,
    finished_ns: Option<u64>,
    outcome: &'static str,
    received_sat: Option<u64>,
    quote_ready_ns: Option<u64>,
    issued_proofs: Option<usize>,
    // A client error or drain cancellation is not a terminal federation outcome.
    requires_reconciliation: bool,
}

fn offset(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_nanos()).expect("benchmark duration fits u64 nanoseconds")
}

async fn receive(
    id: usize,
    wallet: Arc<Wallet>,
    input: Input,
    scheduled_ns: u64,
    started: Arc<AtomicU64>,
    origin: Instant,
    state_dir: PathBuf,
) -> Attempt {
    let started_ns = offset(origin);
    started.store(started_ns + 1, Ordering::Relaxed);
    let mut quote_ready_ns = None;
    let mut issued_proofs = None;
    let operation = async {
        match input {
            Input::Token(token) => Ok(wallet.receive(&token, ReceiveOptions::default()).await?),
            Input::Mint(request) => {
                let method = PaymentMethod::Known(match request.method {
                    MintMethod::Bolt11 => KnownMethod::Bolt11,
                    MintMethod::Onchain => KnownMethod::Onchain,
                });
                let quote = wallet
                    .mint_quote(method, Some(request.amount_sat.into()), None, None)
                    .await?;
                quote_ready_ns = Some(offset(origin));
                // The external payer/miner observes this private file. The SDK
                // has already persisted the quote and its recovery identity.
                fs::write(
                    state_dir.join(format!("request-{id}.json")),
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "id": id, "quote_id": quote.id, "request": quote.request,
                        "method": request.method,
                        "amount_sat": request.amount_sat, "quote_ready_ns": quote_ready_ns,
                    }))?,
                )?;
                // The shared scheduler owns the bounded drain deadline.
                let mut stream = wallet.proof_stream(quote, Default::default(), None);
                let proofs = stream
                    .next()
                    .await
                    .ok_or(Error::Input("mint proof stream ended"))??;
                issued_proofs = Some(proofs.len());
                Ok(proofs.total_amount().map_err(cdk::Error::from)?)
            }
        }
    };
    let result: Result<cdk::Amount> = operation.await;
    if let Err(error) = &result {
        // Keep diagnostics beside durable wallet state, never in public output.
        let _ = fs::write(
            state_dir.join(format!("request-{id}-error.txt")),
            error.to_string(),
        );
    }
    Attempt {
        id,
        scheduled_ns,
        started_ns: Some(started_ns),
        finished_ns: Some(offset(origin)),
        outcome: if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        requires_reconciliation: result.is_err(),
        received_sat: result.ok().map(u64::from),
        quote_ready_ns,
        issued_proofs,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or(Error::Input("private manifest path required"))?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(&path)?)?;
    check(
        (1..=64).contains(&manifest.concurrency),
        "concurrency must be 1–64",
    )?;
    check(
        manifest.window_ms > 0 && manifest.drain_ms > 0,
        "window/drain must be positive",
    )?;
    check(
        manifest.arrival_interval_ms != Some(0),
        "arrival interval must be positive",
    )?;
    check(
        manifest.token_files.is_empty() != manifest.mint_requests.is_empty(),
        "provide either token fixtures or mint requests",
    )?;
    check(
        manifest
            .mint_requests
            .iter()
            .all(|request| request.amount_sat > 0),
        "mint amounts must be positive",
    )?;
    check(
        manifest.window_ms <= 3_600_000 && manifest.drain_ms <= 3_600_000,
        "window/drain must not exceed one hour",
    )?;
    if manifest.trace_performance {
        tracing_subscriber::fmt()
            .with_env_filter("cdk::wallet::federation_performance=debug")
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init()
            .map_err(|_| Error::Input("could not initialize performance tracing"))?;
    }
    let preparation_started = Instant::now();
    // create_dir deliberately fails for existing state, including an earlier run.
    fs::create_dir(&manifest.state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&manifest.state_dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::copy(path, manifest.state_dir.join("manifest.json"))?;
    let config: FederationConfig = serde_json::from_slice(&fs::read(&manifest.public_config)?)?;
    fs::copy(
        &manifest.public_config,
        manifest.state_dir.join("federation-public.json"),
    )?;
    let mint_url = config
        .members
        .values()
        .next()
        .ok_or(Error::Input("empty roster"))?
        .public_mint_url
        .clone();
    let connector = Arc::new(FederatedMintConnector::from_public_config_urls(config)?);
    let mut input_proof_counts = Vec::with_capacity(manifest.token_files.len());
    let mut inputs = manifest
        .token_files
        .iter()
        .map(|path| {
            let encoded = fs::read_to_string(path)?;
            let parsed: Token = encoded.trim().parse().map_err(cdk::Error::from)?;
            check(
                parsed.mint_url().map_err(cdk::Error::from)? == mint_url,
                "fixture mint differs from roster entrypoint",
            )?;
            input_proof_counts.push(match parsed {
                Token::TokenV3(token) => token
                    .token
                    .iter()
                    .map(|entry| entry.proofs.len())
                    .sum::<usize>(),
                Token::TokenV4(token) => token
                    .token
                    .iter()
                    .map(|entry| entry.proofs.len())
                    .sum::<usize>(),
            });
            Ok(Input::Token(encoded.trim().to_owned()))
        })
        .collect::<Result<Vec<_>>>()?;
    inputs.extend(manifest.mint_requests.iter().cloned().map(Input::Mint));

    let wallet_count = if manifest.shared_wallet {
        1
    } else {
        inputs.len()
    };
    let mut wallets = Vec::with_capacity(wallet_count);
    for index in 0..wallet_count {
        let seed = rand::random::<[u8; 64]>();
        fs::write(
            manifest.state_dir.join(format!("wallet-{index}.seed")),
            seed,
        )?;
        let store =
            WalletSqliteDatabase::new(manifest.state_dir.join(format!("wallet-{index}.sqlite")))
                .await
                .map_err(cdk::Error::from)?;
        let wallet = WalletBuilder::new()
            .mint_url(mint_url.clone())
            .unit(CurrencyUnit::Sat)
            .seed(seed)
            .localstore(Arc::new(store))
            .shared_client(connector.clone())
            .build()?;
        if manifest.warm_connections {
            wallet.fetch_mint_info().await?;
        }
        wallets.push(Arc::new(wallet));
    }
    let preparation_ns = offset(preparation_started);
    let origin = Instant::now();
    let window_end = origin + Duration::from_millis(manifest.window_ms);
    let drain_end = window_end + Duration::from_millis(manifest.drain_ms);
    let mut tasks = JoinSet::<Attempt>::new();
    let mut pending = BTreeMap::new();
    let mut attempts = Vec::with_capacity(inputs.len());
    let request_count = inputs.len();
    for (id, input) in inputs.into_iter().enumerate() {
        let scheduled_ns = match manifest.arrival_interval_ms {
            Some(interval) => {
                let arrival = origin
                    + Duration::from_millis(
                        interval
                            .checked_mul(id as u64)
                            .ok_or(Error::Input("arrival overflow"))?,
                    );
                if arrival >= window_end {
                    break;
                }
                sleep_until(arrival).await;
                u64::try_from(arrival.duration_since(origin).as_nanos())?
            }
            None => {
                if tasks.len() == manifest.concurrency {
                    match timeout_at(window_end, tasks.join_next()).await {
                        Ok(Some(result)) => {
                            let attempt = result?;
                            pending.remove(&attempt.id);
                            attempts.push(attempt);
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
                if Instant::now() >= window_end {
                    break;
                }
                offset(origin)
            }
        };
        while let Some(result) = tasks.try_join_next() {
            let attempt = result?;
            pending.remove(&attempt.id);
            attempts.push(attempt);
        }
        if tasks.len() == manifest.concurrency || Instant::now() >= window_end {
            attempts.push(Attempt {
                id,
                scheduled_ns,
                started_ns: None,
                finished_ns: Some(offset(origin)),
                outcome: "rejected",
                received_sat: None,
                quote_ready_ns: None,
                issued_proofs: None,
                requires_reconciliation: false,
            });
            continue;
        }
        let started = Arc::new(AtomicU64::new(0));
        pending.insert(id, (scheduled_ns, started.clone()));
        let wallet = wallets[if manifest.shared_wallet { 0 } else { id }].clone();
        tasks.spawn(receive(
            id,
            wallet,
            input,
            scheduled_ns,
            started,
            origin,
            manifest.state_dir.clone(),
        ));
    }
    while !tasks.is_empty() {
        match timeout_at(drain_end, tasks.join_next()).await {
            Ok(Some(result)) => {
                let attempt = result?;
                pending.remove(&attempt.id);
                attempts.push(attempt);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    tasks.shutdown().await;
    for (id, (scheduled_ns, started)) in pending {
        attempts.push(Attempt {
            id,
            scheduled_ns,
            started_ns: started.load(Ordering::Relaxed).checked_sub(1),
            finished_ns: None,
            outcome: "pending",
            received_sat: None,
            quote_ready_ns: None,
            issued_proofs: None,
            requires_reconciliation: true,
        });
    }
    sleep_until(window_end).await;
    let actual_observation_ns = offset(origin);
    attempts.sort_by_key(|attempt| attempt.id);
    let mut wallet_balances_sat = Vec::with_capacity(wallets.len());
    let mut wallet_proof_counts = Vec::with_capacity(wallets.len());
    for wallet in &wallets {
        wallet_balances_sat.push(u64::from(wallet.total_balance().await?));
        wallet_proof_counts.push(wallet.get_unspent_proofs().await?.len());
    }
    if attempts
        .iter()
        .all(|attempt| !attempt.requires_reconciliation)
    {
        check(
            wallet_balances_sat.iter().sum::<u64>()
                == attempts
                    .iter()
                    .filter_map(|attempt| attempt.received_sat)
                    .sum::<u64>(),
            "recipient balances differ from confirmed receipts",
        )?;
    }
    let report = serde_json::json!({
        "driver_version": 2, "window_ns": manifest.window_ms * 1_000_000,
        "preparation_ns": preparation_ns, "actual_observation_ns": actual_observation_ns,
        "concurrency": manifest.concurrency, "arrival_interval_ms": manifest.arrival_interval_ms,
        "queue_capacity": 0, "shared_wallet": manifest.shared_wallet,
        "warm_connections": manifest.warm_connections,
        "trace_performance": manifest.trace_performance,
        "mint_requests": manifest.mint_requests,
        "input_proof_counts": input_proof_counts, "wallet_balances_sat": wallet_balances_sat,
        "wallet_proof_counts": wallet_proof_counts,
        "requests_not_offered": request_count - attempts.len(), "attempts": attempts,
    });
    fs::write(
        manifest.state_dir.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
