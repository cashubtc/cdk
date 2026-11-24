//! Pending states created through the released wallet API and recovered after upgrade.
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::{
    CheckStateRequest, MeltQuoteState, MintQuoteState, PaymentMethod, ProofsMethods, State,
};
use cdk::wallet::{HttpClient, MeltOutcome, MintConnector, SendOptions, Wallet};
use cdk::Amount;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{MeltSagaState, SendSagaState, WalletSagaState};
use cdk_fake_wallet::create_fake_invoice;
use cdk_sqlite::WalletSqliteDatabase;
use serde::{Deserialize, Serialize};

use crate::{fund, proof_identity, Operations};

async fn melt_state(url: &str, quote: &str) -> Result<MeltQuoteState> {
    let client = HttpClient::new(url.parse()?, None);
    Ok(client
        .get_melt_quote_status(PaymentMethod::BOLT11, quote)
        .await?
        .state())
}

#[derive(Debug, Serialize, Deserialize)]
struct Held {
    name: String,
    seed: u8,
    saga: Option<uuid::Uuid>,
    quote: Option<String>,
    payment_hash: Option<String>,
    spendable: BTreeSet<String>,
    reserved: BTreeSet<String>,
    pending: BTreeSet<String>,
}

async fn open(
    root: &Path,
    name: &str,
    seed: u8,
    url: &str,
) -> Result<(Wallet, Arc<WalletSqliteDatabase>)> {
    let db = Arc::new(WalletSqliteDatabase::new(root.join(format!("{name}.sqlite"))).await?);
    let wallet = Wallet::new(
        url,
        cdk::nuts::CurrencyUnit::Sat,
        db.clone(),
        [seed; 64],
        Some(4),
    )?;
    Ok((wallet, db))
}

async fn record(
    wallet: &Wallet,
    name: &str,
    seed: u8,
    saga: Option<uuid::Uuid>,
    quote: Option<String>,
    payment_hash: Option<String>,
) -> Result<Held> {
    Ok(Held {
        name: name.to_owned(),
        seed,
        saga,
        quote,
        payment_hash,
        spendable: wallet
            .get_unspent_proofs()
            .await?
            .iter()
            .map(proof_identity)
            .collect(),
        reserved: wallet
            .get_reserved_proofs()
            .await?
            .iter()
            .map(proof_identity)
            .collect(),
        pending: wallet
            .get_pending_proofs()
            .await?
            .iter()
            .map(proof_identity)
            .collect(),
    })
}

async fn saga_state(db: &WalletSqliteDatabase, held: &Held) -> Result<()> {
    let Some(id) = held.saga else { return Ok(()) };
    let saga = db.get_saga(&id).await?.expect("persisted pending saga");
    let valid = match held.name.as_str() {
        "reserved-send" => matches!(
            saga.state,
            WalletSagaState::Send(SendSagaState::ProofsReserved)
        ),
        "reserved-melt" => matches!(
            saga.state,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved)
        ),
        _ => matches!(
            saga.state,
            WalletSagaState::Melt(MeltSagaState::PaymentPending | MeltSagaState::MeltRequested)
        ),
    };
    ensure!(valid, "{}: unexpected persisted saga state", held.name);
    Ok(())
}

pub(crate) async fn seed(root: &Path, url: &str, operations: &mut Operations) -> Result<()> {
    let mut held = Vec::new();
    for (name, seed) in [
        ("reserved-send", 3),
        ("reserved-melt", 4),
        ("pending-paid", 5),
        ("pending-failed", 6),
    ] {
        let (wallet, db) = open(root, name, seed, url).await?;
        fund(&wallet, 128).await?;
        operations.completed_money_operations += 1;
        operations.quotes_created += 1;
        let entry = match name {
            "reserved-send" => {
                let prepared = wallet
                    .prepare_send(13.into(), SendOptions::default())
                    .await?;
                let id = prepared.operation_id();
                drop(prepared);
                ensure!(
                    !wallet.get_reserved_proofs().await?.is_empty(),
                    "prepared send did not reserve proofs"
                );
                record(&wallet, name, seed, Some(id), None, None).await?
            }
            _ => {
                let description = match name {
                    "pending-paid" => "upgrade-pending-paid",
                    "pending-failed" => "upgrade-pending-failed",
                    _ => "prepared melt",
                };
                let invoice = create_fake_invoice(3_000, description.to_owned());
                let hash = invoice.payment_hash().to_string();
                let quote = wallet
                    .melt_quote(PaymentMethod::BOLT11, invoice.to_string(), None, None)
                    .await?;
                operations.quotes_created += 1;
                let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
                let id = prepared.operation_id();
                match name {
                    "reserved-melt" => {
                        drop(prepared);
                        ensure!(
                            !wallet.get_reserved_proofs().await?.is_empty(),
                            "prepared melt did not reserve proofs"
                        );
                    }
                    _ => {
                        match tokio::time::timeout(
                            Duration::from_secs(30),
                            prepared.confirm_prefer_async(),
                        )
                        .await??
                        {
                            MeltOutcome::Pending(pending) => drop(pending),
                            MeltOutcome::Paid(_) => {
                                anyhow::bail!("controlled payment finished before upgrade")
                            }
                        }
                        let deadline = Instant::now() + Duration::from_secs(10);
                        let calls = root.join("payments").join(format!("outgoing-{hash}.json"));
                        while !calls.exists() {
                            ensure!(
                                Instant::now() < deadline,
                                "payment dispatch did not reach backend"
                            );
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        ensure!(
                            melt_state(url, &quote.id).await? == MeltQuoteState::Pending,
                            "melt did not remain PENDING"
                        );
                    }
                }
                record(&wallet, name, seed, Some(id), Some(quote.id), Some(hash)).await?
            }
        };
        saga_state(&db, &entry).await?;
        held.push(entry);
    }
    let (wallet, _) = open(root, "unpaid-quote", 7, url).await?;
    let quote = wallet
        .mint_quote(
            PaymentMethod::BOLT11,
            Some(23.into()),
            Some("upgrade-unpaid".to_owned()),
            None,
        )
        .await?;
    ensure!(
        wallet.fetch_mint_quote(&quote.id, None).await?.state == MintQuoteState::Unpaid,
        "controlled quote was paid before upgrade"
    );
    held.push(record(&wallet, "unpaid-quote", 7, None, Some(quote.id), None).await?);
    operations.quotes_created += 1;
    fs::write(
        root.join("held-states.json"),
        serde_json::to_vec_pretty(&held)?,
    )?;
    operations.save(root)?;
    println!("Held five pending states before upgrade");
    Ok(())
}

pub(crate) async fn verify(root: &Path, url: &str, operations: &mut Operations) -> Result<()> {
    let held: Vec<Held> = serde_json::from_slice(&fs::read(root.join("held-states.json"))?)?;
    for entry in held {
        let (wallet, db) = open(root, &entry.name, entry.seed, url).await?;
        let actual = record(
            &wallet,
            &entry.name,
            entry.seed,
            entry.saga,
            entry.quote.clone(),
            entry.payment_hash.clone(),
        )
        .await?;
        ensure!(
            actual.spendable == entry.spendable
                && actual.reserved == entry.reserved
                && actual.pending == entry.pending,
            "{}: held proof state changed during upgrade",
            entry.name
        );
        saga_state(&db, &entry).await?;
        match entry.name.as_str() {
            "pending-paid" | "pending-failed" => {
                let quote = entry.quote.as_ref().expect("melt quote");
                ensure!(
                    melt_state(url, quote).await? == MeltQuoteState::Pending,
                    "pending melt became terminal before release"
                );
                let locked = wallet
                    .get_proofs_by_states(vec![State::Reserved, State::Pending])
                    .await?;
                ensure!(!locked.is_empty(), "pending melt lost its locked inputs");
                let client = HttpClient::new(url.parse()?, None);
                let states = client
                    .post_check_state(CheckStateRequest { ys: locked.ys()? })
                    .await?
                    .states;
                ensure!(
                    states.len() == locked.len()
                        && states.iter().all(|state| state.state == State::Pending),
                    "mint did not preserve PENDING inputs"
                );
                let recovery = wallet.recover_incomplete_sagas().await?;
                ensure!(
                    recovery.failed == 0 && recovery.compensated == 0 && recovery.skipped == 1,
                    "wallet released or finalized an unresolved payment"
                );
                saga_state(&db, &entry).await?;
                ensure!(
                    wallet.total_balance().await? == Amount::from(124),
                    "pending payment restored spendable inputs prematurely"
                );
            }
            "unpaid-quote" => {
                ensure!(
                    wallet
                        .fetch_mint_quote(entry.quote.as_ref().expect("mint quote"), None)
                        .await?
                        .state
                        == MintQuoteState::Unpaid,
                    "UNPAID quote changed before release"
                );
                ensure!(
                    wallet.total_balance().await? == Amount::ZERO,
                    "unpaid quote credited tokens"
                );
            }
            _ => {}
        }
    }
    operations.pending_upgrade_checks += 1;
    operations.save(root)?;
    Ok(())
}

pub(crate) async fn finish(root: &Path, url: &str, operations: &mut Operations) -> Result<()> {
    let held: Vec<Held> = serde_json::from_slice(&fs::read(root.join("held-states.json"))?)?;
    for entry in held {
        let (wallet, db) = open(root, &entry.name, entry.seed, url).await?;
        match entry.name.as_str() {
            "unpaid-quote" => {
                let id = entry.quote.as_ref().expect("quote id");
                let deadline = Instant::now() + Duration::from_secs(30);
                while wallet.fetch_mint_quote(id, None).await?.state != MintQuoteState::Paid {
                    ensure!(Instant::now() < deadline, "released invoice not PAID");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                ensure!(
                    wallet
                        .mint(id, SplitTarget::default(), None)
                        .await?
                        .total_amount()?
                        == Amount::from(23),
                    "unpaid pre-upgrade quote cannot be claimed"
                );
                let _retry = wallet.mint(id, SplitTarget::default(), None).await;
                ensure!(
                    wallet.total_balance().await? == Amount::from(23),
                    "quote issued twice"
                );
                operations.unpaid_quote_completions += 1;
                operations.completed_money_operations += 1;
                operations.quote_replay_checks += 1;
            }
            "reserved-send" | "reserved-melt" => {
                let recovery = wallet.recover_incomplete_sagas().await?;
                ensure!(
                    recovery.failed == 0 && recovery.compensated == 1,
                    "prepared operation not compensated"
                );
                ensure!(
                    wallet.total_balance().await? == Amount::from(128),
                    "prepared recovery lost funds"
                );
                ensure!(
                    wallet.get_reserved_proofs().await?.is_empty(),
                    "prepared recovery leaked reservations"
                );
                let expected: BTreeSet<String> =
                    entry.spendable.union(&entry.reserved).cloned().collect();
                ensure!(
                    wallet
                        .get_unspent_proofs()
                        .await?
                        .iter()
                        .map(proof_identity)
                        .collect::<BTreeSet<_>>()
                        == expected,
                    "prepared recovery changed proof identities"
                );
                match entry.name.as_str() {
                    "reserved-send" => {
                        wallet
                            .prepare_send(13.into(), SendOptions::default())
                            .await?
                            .cancel()
                            .await?;
                        operations.prepared_send_recoveries += 1;
                    }
                    _ => {
                        wallet
                            .prepare_melt(entry.quote.as_ref().expect("melt quote"), HashMap::new())
                            .await?
                            .cancel()
                            .await?;
                        operations.prepared_melt_recoveries += 1;
                    }
                }
            }
            _ => {
                let id = entry.quote.as_ref().expect("melt quote");
                let paid = entry.name == "pending-paid";
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let state = melt_state(url, id).await?;
                    if matches!(
                        state,
                        MeltQuoteState::Paid | MeltQuoteState::Unpaid | MeltQuoteState::Failed
                    ) {
                        ensure!(
                            matches!(state, MeltQuoteState::Paid) == paid,
                            "pending melt recovered to wrong outcome"
                        );
                        break;
                    }
                    ensure!(Instant::now() < deadline, "released melt never settled");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                let recovery = wallet.recover_incomplete_sagas().await?;
                ensure!(recovery.failed == 0, "pending melt recovery failed");
                ensure!(
                    wallet.total_balance().await? == Amount::from(if paid { 124 } else { 128 }),
                    "pending melt lost or double credited funds"
                );
                ensure!(
                    wallet.get_pending_proofs().await?.is_empty()
                        && wallet.get_reserved_proofs().await?.is_empty(),
                    "pending melt leaked locked proofs"
                );
                let calls = root.join("payments").join(format!(
                    "outgoing-{}.calls",
                    entry.payment_hash.as_ref().expect("payment hash")
                ));
                ensure!(
                    fs::read_to_string(calls)?.lines().count() == 1,
                    "payment dispatched again during upgrade"
                );
                if paid {
                    operations.pending_melt_paid_recoveries += 1;
                    operations.completed_money_operations += 1;
                } else {
                    operations.pending_melt_failed_recoveries += 1;
                }
            }
        }
        if let Some(id) = entry.saga {
            ensure!(
                db.get_saga(&id).await?.is_none(),
                "resolved operation retained its saga"
            );
        }
        let (reopened, _) = open(root, &entry.name, entry.seed, url).await?;
        ensure!(
            reopened.recover_incomplete_sagas().await?.is_empty(),
            "recovery was not idempotent after reopening wallet"
        );
        let expected = match entry.name.as_str() {
            "pending-paid" => 124,
            "unpaid-quote" => 23,
            _ => 128,
        };
        ensure!(
            reopened.total_balance().await? == Amount::from(expected),
            "recovered balance did not survive reopening wallet"
        );
        println!("Recovered pending upgrade state: {}", entry.name);
    }
    fs::write(root.join("pending-completed"), b"complete")?;
    operations.save(root)?;
    Ok(())
}
