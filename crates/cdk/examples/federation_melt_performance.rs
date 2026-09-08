//! Serial persistent-SDK payment measurements using a prepared private CLI wallet.
//! The manifest and output directory contain payment identities: keep them private.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::{CurrencyUnit, MeltQuoteState, PaymentMethod};
use cdk::wallet::{FederatedMintConnector, WalletBuilder};
use cdk::Amount;
use cdk_common::federation::FederationConfig;
use cdk_sqlite::WalletSqliteDatabase;
use serde::Deserialize;
use tokio::time::{timeout_at, Instant};

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
    Mnemonic(#[from] bip39::Error),
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
    wallet_dir: PathBuf,
    output_dir: PathBuf,
    requests: Vec<Request>,
    window_ms: u64,
    drain_ms: u64,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Onchain {
        address: String,
        amount_sat: u64,
        fee_option: usize,
        expected_fee_reserve_sat: Option<u64>,
    },
    Bolt11 {
        invoice: String,
    },
}

fn offset(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_nanos()).expect("bounded benchmark duration")
}

#[tokio::main]
async fn main() -> Result<()> {
    let manifest_path = std::env::args()
        .nth(1)
        .ok_or(Error::Input("private manifest required"))?;
    let encoded = fs::read(manifest_path)?;
    let manifest: Manifest = serde_json::from_slice(&encoded)?;
    check(!manifest.requests.is_empty(), "payment requests required")?;
    check(
        (1..=3_600_000).contains(&manifest.window_ms)
            && (1..=3_600_000).contains(&manifest.drain_ms),
        "window and drain must be positive and bounded to one hour",
    )?;
    let database = manifest.wallet_dir.join("cdk-cli.sqlite");
    check(
        database.is_file(),
        "a funded wallet database must already exist",
    )?;
    fs::create_dir(&manifest.output_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&manifest.output_dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::write(manifest.output_dir.join("manifest.json"), encoded)?;
    let preparation = Instant::now();
    let config: FederationConfig = serde_json::from_slice(&fs::read(&manifest.public_config)?)?;
    let mint_url = config
        .members
        .values()
        .next()
        .ok_or(Error::Input("empty roster"))?
        .public_mint_url
        .clone();
    let mnemonic: bip39::Mnemonic = fs::read_to_string(manifest.wallet_dir.join("seed"))?
        .trim()
        .parse()?;
    let wallet = WalletBuilder::new()
        .mint_url(mint_url)
        .unit(CurrencyUnit::Sat)
        .seed(mnemonic.to_seed_normalized(""))
        .localstore(Arc::new(
            WalletSqliteDatabase::new(database)
                .await
                .map_err(cdk::Error::from)?,
        ))
        .shared_client(Arc::new(FederatedMintConnector::from_public_config_urls(
            config,
        )?))
        .build()?;
    let recovery = wallet.recover_incomplete_sagas().await?;
    check(
        recovery.failed == 0 && recovery.skipped == 0,
        "resolve pending recovery before measurement",
    )?;
    wallet.fetch_mint_info().await?;
    let initial_balance_sat = u64::from(wallet.total_balance().await?);
    let initial_proofs = wallet.get_unspent_proofs().await?.len();
    let preparation_ns = offset(preparation);
    let origin = Instant::now();
    let window_end = origin + Duration::from_millis(manifest.window_ms);
    let drain_end = window_end + Duration::from_millis(manifest.drain_ms);
    let mut attempts = Vec::new();
    for (id, request) in manifest.requests.iter().enumerate() {
        if Instant::now() >= window_end {
            break;
        }
        let scheduled_ns = offset(origin);
        let mut details = serde_json::json!({"id": id, "scheduled_ns": scheduled_ns});
        let operation = async {
            let quote = match request {
                Request::Onchain {
                    address,
                    amount_sat,
                    fee_option,
                    ..
                } => {
                    let mut options = wallet
                        .quote_onchain_melt_options(address, Amount::from(*amount_sat), None)
                        .await?;
                    check(
                        *fee_option < options.len(),
                        "requested fee option is unavailable",
                    )?;
                    wallet
                        .select_onchain_melt_quote(options.remove(*fee_option))
                        .await?
                }
                Request::Bolt11 { invoice } => {
                    wallet
                        .melt_quote(PaymentMethod::BOLT11, invoice, None, None)
                        .await?
                }
            };
            details["quote_id"] = serde_json::json!(quote.id);
            details["quote_ready_ns"] = serde_json::json!(offset(origin));
            details["fee_reserve_sat"] = serde_json::json!(u64::from(quote.fee_reserve));
            fs::write(
                manifest.output_dir.join(format!("request-{id}.json")),
                serde_json::to_vec_pretty(&details)?,
            )?;
            if let Request::Onchain {
                expected_fee_reserve_sat: Some(expected),
                ..
            } = request
            {
                check(
                    u64::from(quote.fee_reserve) == *expected,
                    "fee reserve differs from the comparison manifest; payment not initiated",
                )?;
            }
            let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
            details["operation_id"] = serde_json::json!(prepared.operation_id());
            details["selected_proofs"] = serde_json::json!(prepared.proofs().len());
            details["proofs_to_swap"] = serde_json::json!(prepared.proofs_to_swap().len());
            details["prepared_ns"] = serde_json::json!(offset(origin));
            fs::write(
                manifest.output_dir.join(format!("request-{id}.json")),
                serde_json::to_vec_pretty(&details)?,
            )?;
            let melted = prepared.confirm().await?;
            details["amount_sat"] = serde_json::json!(u64::from(melted.amount()));
            details["fee_paid_sat"] = serde_json::json!(u64::from(melted.fee_paid()));
            details["payment_proof"] = serde_json::json!(melted.payment_proof());
            Ok::<_, Error>(melted.state())
        };
        let result = timeout_at(drain_end, operation).await;
        let (outcome, finished_ns) = match result {
            Ok(Ok(MeltQuoteState::Paid)) => ("completed", Some(offset(origin))),
            Ok(Ok(_)) | Err(_) => ("pending", None),
            Ok(Err(error)) => {
                details["error"] = serde_json::json!(error.to_string());
                ("failed", Some(offset(origin)))
            }
        };
        let needs_reconciliation = outcome != "completed";
        details["outcome"] = serde_json::json!(outcome);
        fs::write(
            manifest.output_dir.join(format!("request-{id}.json")),
            serde_json::to_vec_pretty(&details)?,
        )?;
        attempts.push(serde_json::json!({
            "id": id, "scheduled_ns": scheduled_ns, "started_ns": scheduled_ns,
            "finished_ns": finished_ns, "outcome": outcome,
            "requires_reconciliation": needs_reconciliation,
            "quote_ready_ns": details.get("quote_ready_ns"),
            "prepared_ns": details.get("prepared_ns"),
            "fee_reserve_sat": details.get("fee_reserve_sat"),
            "selected_proofs": details.get("selected_proofs"),
            "proofs_to_swap": details.get("proofs_to_swap"),
            "amount_sat": details.get("amount_sat"), "fee_paid_sat": details.get("fee_paid_sat"),
        }));
        // Keep ambiguous state for recovery; do not initiate more payments after failure.
        if needs_reconciliation {
            break;
        }
    }
    let active_observation_ns = offset(origin);
    let final_balance_sat = u64::from(wallet.total_balance().await?);
    let report = serde_json::json!({
        "driver_version": 1, "concurrency": 1, "shared_wallet": true,
        "workload": "bounded serial fixed-work sequence",
        "window_ns": active_observation_ns.min(manifest.window_ms * 1_000_000),
        "maximum_window_ns": manifest.window_ms * 1_000_000, "preparation_ns": preparation_ns,
        "active_observation_ns": active_observation_ns,
        "initial_balance_sat": initial_balance_sat, "final_balance_sat": final_balance_sat,
        "initial_wallet_proofs": initial_proofs, "requests_not_offered": manifest.requests.len() - attempts.len(),
        "attempts": attempts,
    });
    fs::write(
        manifest.output_dir.join("results.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
