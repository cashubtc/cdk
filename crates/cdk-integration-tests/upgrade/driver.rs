//! The same workload is compiled against each side of an upgrade.
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::{
    CheckStateRequest, CurrencyUnit, MeltQuoteState, PaymentMethod, ProofsMethods, State, Token,
};
use cdk::wallet::{HttpClient, MintConnector, ReceiveOptions, SendOptions, Wallet};
use cdk::Amount;
use cdk_fake_wallet::create_fake_invoice;
use cdk_sqlite::WalletSqliteDatabase;
use serde::{Deserialize, Serialize};

mod pending;

#[derive(Debug, Serialize, Deserialize)]
struct WalletConfig {
    seed: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_proof_count: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    balance: u64,
    proofs: BTreeSet<String>,
    pending_spent: BTreeSet<String>,
    transactions: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Operations {
    workload_rounds: u64,
    completed_money_operations: u64,
    quotes_created: u64,
    double_spend_checks: u64,
    quote_replay_checks: u64,
    pending_send_reconciliations: u64,
    seed_restores: u64,
    pending_upgrade_checks: u64,
    prepared_send_recoveries: u64,
    prepared_melt_recoveries: u64,
    pending_melt_paid_recoveries: u64,
    pending_melt_failed_recoveries: u64,
    unpaid_quote_completions: u64,
}

impl Operations {
    fn save(&self, root: &Path) -> Result<()> {
        fs::write(
            root.join("operations.json"),
            serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }
}

fn proof_identity(proof: &cdk::nuts::Proof) -> String {
    format!(
        "{}:{}:{}:{}",
        proof.keyset_id, proof.amount, proof.secret, proof.c
    )
}

async fn open(root: &Path, name: &str, url: &str) -> Result<Wallet> {
    let config: WalletConfig =
        serde_json::from_slice(&fs::read(root.join(format!("{name}.json")))?)?;
    let seed: [u8; 64] = config
        .seed
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid wallet seed"))?;
    let db = WalletSqliteDatabase::new(root.join(format!("{name}.sqlite"))).await?;
    Ok(Wallet::new(
        url,
        CurrencyUnit::Sat,
        Arc::new(db),
        seed,
        config.target_proof_count,
    )?)
}

async fn snapshot(wallet: &Wallet) -> Result<Snapshot> {
    let mut proofs = BTreeSet::new();
    for proof in wallet.get_unspent_proofs().await? {
        // Compare cryptographic state across versions, independent of optional
        // fields a future serializer might add.
        proofs.insert(proof_identity(&proof));
    }
    let pending_spent = wallet
        .get_pending_spent_proofs()
        .await?
        .iter()
        .map(proof_identity)
        .collect();
    let mut transactions = Vec::new();
    for mut transaction in wallet.list_transactions(None).await? {
        transaction.ys.sort();
        // The transaction ID algorithm intentionally changed to use saga IDs.
        // Compare the stored history content rather than that computed ID or
        // fields newly introduced by the upgraded version (such as status).
        transactions.push(serde_json::to_string(&serde_json::json!({
            "mint_url": transaction.mint_url,
            "direction": transaction.direction,
            "amount": transaction.amount,
            "fee": transaction.fee,
            "unit": transaction.unit,
            "ys": transaction.ys,
            "timestamp": transaction.timestamp,
            "memo": transaction.memo,
            "metadata": transaction.metadata,
            "quote_id": transaction.quote_id,
            "payment_request": transaction.payment_request,
            "payment_proof": transaction.payment_proof,
            "payment_method": transaction.payment_method,
            "saga_id": transaction.saga_id,
        }))?);
    }
    transactions.sort();
    Ok(Snapshot {
        balance: u64::from(wallet.total_balance().await?),
        proofs,
        pending_spent,
        transactions,
    })
}

async fn checkpoint(root: &Path, name: &str, wallet: &Wallet) -> Result<()> {
    fs::write(
        root.join(format!("{name}.snapshot.json")),
        serde_json::to_vec_pretty(&snapshot(wallet).await?)?,
    )?;
    Ok(())
}

async fn verify(root: &Path, name: &str, wallet: &Wallet) -> Result<()> {
    let expected: Snapshot =
        serde_json::from_slice(&fs::read(root.join(format!("{name}.snapshot.json")))?)?;
    let actual = snapshot(wallet).await?;
    ensure!(
        actual.balance == expected.balance,
        "{name}: balance changed during upgrade/restart"
    );
    ensure!(
        actual.proofs == expected.proofs,
        "{name}: unspent proofs changed during upgrade/restart"
    );
    ensure!(
        actual.transactions == expected.transactions,
        "{name}: transaction history changed during upgrade/restart"
    );
    ensure!(
        actual.pending_spent == expected.pending_spent,
        "{name}: pending sent proofs changed during upgrade/restart"
    );
    Ok(())
}

async fn fund(wallet: &Wallet, amount: u64) -> Result<Vec<String>> {
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount.into()), None, None)
        .await?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    ensure!(
        quote.expiry > now + 7000 && quote.expiry <= now + 7300,
        "configured mint quote TTL changed"
    );
    let proofs = wallet
        .wait_and_mint_quote(quote, SplitTarget::default(), None, Duration::from_secs(30))
        .await?;
    ensure!(
        proofs.total_amount()? == Amount::from(amount),
        "incorrect mint amount"
    );
    Ok(proofs.into_iter().map(|p| p.secret.to_string()).collect())
}

async fn verify_spent(url: &str, encoded: &str) -> Result<()> {
    let client = HttpClient::new(url.parse()?, None);
    let keysets = client.get_mint_keysets().await?.keysets;
    let proofs = Token::from_str(encoded)?.proofs(&keysets)?;
    let ys = proofs.ys()?;
    let expected = ys.len();
    let states = client
        .post_check_state(CheckStateRequest { ys })
        .await?
        .states;
    ensure!(
        states.len() == expected && states.iter().all(|s| s.state == State::Spent),
        "mint lost spent proof state"
    );
    Ok(())
}

async fn verify_unredeemed(wallet: &Wallet, url: &str, encoded: &str) -> Result<()> {
    let client = HttpClient::new(url.parse()?, None);
    let keysets = client.get_mint_keysets().await?.keysets;
    let proofs = Token::from_str(encoded)?.proofs(&keysets)?;
    let pending: BTreeSet<String> = wallet
        .get_pending_spent_proofs()
        .await?
        .iter()
        .map(proof_identity)
        .collect();
    ensure!(
        proofs
            .iter()
            .all(|proof| pending.contains(&proof_identity(proof))),
        "unredeemed token lost its wallet PendingSpent proofs"
    );
    let expected = proofs.len();
    let states = client
        .post_check_state(CheckStateRequest { ys: proofs.ys()? })
        .await?
        .states;
    ensure!(
        states.len() == expected && states.iter().all(|state| state.state == State::Unspent),
        "unredeemed token is not spendable at the mint"
    );
    Ok(())
}

async fn reconcile_sent_token(wallet: &Wallet, url: &str, encoded: &str) -> Result<()> {
    let client = HttpClient::new(url.parse()?, None);
    let keysets = client.get_mint_keysets().await?.keysets;
    let proofs = Token::from_str(encoded)?.proofs(&keysets)?;
    let identities: BTreeSet<String> = proofs.iter().map(proof_identity).collect();
    let states = wallet.check_proofs_spent(proofs).await?;
    ensure!(
        states.len() == identities.len() && states.iter().all(|state| state.state == State::Spent),
        "redeemed pre-upgrade token was not marked spent"
    );
    ensure!(
        wallet
            .get_pending_spent_proofs()
            .await?
            .iter()
            .all(|proof| !identities.contains(&proof_identity(proof))),
        "redeemed pre-upgrade token remained pending in the sender wallet"
    );
    Ok(())
}

async fn send(wallet: &Wallet, amount: u64) -> Result<String> {
    Ok(wallet
        .prepare_send(amount.into(), SendOptions::default())
        .await?
        .confirm(None)
        .await?
        .to_string())
}

async fn workload(
    root: &Path,
    a: &Wallet,
    b: &Wallet,
    rounds: usize,
    operations: &mut Operations,
) -> Result<()> {
    let seen_path = root.join("secrets.json");
    let mut seen: BTreeSet<String> = if seen_path.exists() {
        serde_json::from_slice(&fs::read(&seen_path)?)?
    } else {
        BTreeSet::new()
    };
    for i in 0..rounds {
        let before = a.total_balance().await?;
        for secret in fund(a, 128 + i as u64).await? {
            ensure!(
                seen.insert(secret),
                "wallet derivation counter reused a mint secret"
            );
        }
        ensure!(
            a.total_balance().await? == before + Amount::from(128 + i as u64),
            "mint balance mismatch"
        );
        let amount = 7 + i as u64 % 13;
        let funded_balance = a.total_balance().await?;
        let bob_balance = b.total_balance().await?;
        let token = send(a, amount).await?;
        ensure!(
            a.total_balance().await? == funded_balance - Amount::from(amount),
            "send lost change or debited the wrong amount"
        );
        ensure!(
            b.receive(&token, ReceiveOptions::default()).await? == Amount::from(amount),
            "send/receive amount mismatch"
        );
        ensure!(
            b.total_balance().await? == bob_balance + Amount::from(amount),
            "receive credited the wrong balance"
        );
        fs::write(root.join("spent-token.txt"), &token)?;
        ensure!(
            b.receive(&token, ReceiveOptions::default()).await.is_err(),
            "spent token accepted twice"
        );
        let returned = send(b, amount).await?;
        ensure!(
            a.receive(&returned, ReceiveOptions::default()).await? == Amount::from(amount),
            "return amount mismatch"
        );
        ensure!(
            a.total_balance().await? == funded_balance && b.total_balance().await? == bob_balance,
            "send/receive round trip did not conserve balances"
        );
        let quote = a
            .melt_quote(
                PaymentMethod::BOLT11,
                create_fake_invoice(3_000, "upgrade test".to_owned()).to_string(),
                None,
                None,
            )
            .await?;
        let before = a.total_balance().await?;
        let prepared = a.prepare_melt(&quote.id, HashMap::new()).await?;
        let paid = tokio::time::timeout(Duration::from_secs(30), prepared.confirm())
            .await
            .context("melt completion timed out")??;
        ensure!(paid.state() == MeltQuoteState::Paid, "melt not paid");
        ensure!(
            a.total_balance().await? == before - paid.amount() - paid.fee_paid(),
            "melt balance/fee mismatch"
        );
        fs::write(&seen_path, serde_json::to_vec(&seen)?)?;
        operations.workload_rounds += 1;
        operations.completed_money_operations += 6;
        operations.quotes_created += 2;
        operations.double_spend_checks += 1;
        operations.save(root)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "usage: upgrade-driver PHASE DIRECTORY URL ROUNDS"
    );
    let root = Path::new(&args[2]);
    let rounds: usize = args[4].parse()?;
    let mut operations: Operations = match root.join("operations.json").exists() {
        true => serde_json::from_slice(&fs::read(root.join("operations.json"))?)?,
        false => Operations::default(),
    };
    if args[1] == "seed" {
        for (name, byte) in [("alice", 1), ("bob", 2)] {
            fs::write(
                root.join(format!("{name}.json")),
                serde_json::to_vec(&WalletConfig {
                    seed: vec![byte; 64],
                    target_proof_count: match root.join("wallet-defaults").exists() {
                        true => None,
                        false => Some(4),
                    },
                })?,
            )?;
        }
    }
    let a = open(root, "alice", &args[3])
        .await
        .context("open Alice wallet")?;
    let b = open(root, "bob", &args[3])
        .await
        .context("open Bob wallet")?;
    if args[1] != "seed" {
        verify(root, "alice", &a).await?;
        verify(root, "bob", &b).await?;
        let spent = fs::read_to_string(root.join("spent-token.txt"))?;
        verify_spent(&args[3], &spent).await?;
        if !root.join("handoff-completed").exists() {
            verify_unredeemed(
                &a,
                &args[3],
                &fs::read_to_string(root.join("unredeemed-token.txt"))?,
            )
            .await?;
        }
    }
    match args[1].as_str() {
        "seed" => {
            workload(root, &a, &b, rounds, &mut operations).await?;
            let token = send(&a, 17).await?;
            fs::write(root.join("unredeemed-token.txt"), &token)?;
            verify_unredeemed(&a, &args[3], &token).await?;
            let quote = a
                .mint_quote(PaymentMethod::BOLT11, Some(Amount::from(31)), None, None)
                .await?;
            a.wait_for_payment(&quote, Duration::from_secs(30)).await?;
            fs::write(root.join("unclaimed-quote.txt"), quote.id)?;
            operations.completed_money_operations += 1;
            operations.quotes_created += 1;
        }
        "exercise" => workload(root, &a, &b, rounds, &mut operations).await?,
        "seed-pending" => pending::seed(root, &args[3], &mut operations).await?,
        "verify-pending" => pending::verify(root, &args[3], &mut operations).await?,
        "finish-pending" => pending::finish(root, &args[3], &mut operations).await?,
        "finish" => {
            let token = fs::read_to_string(root.join("unredeemed-token.txt"))?;
            ensure!(
                b.receive(&token, ReceiveOptions::default()).await? == Amount::from(17),
                "pre-upgrade token cannot be redeemed"
            );
            reconcile_sent_token(&a, &args[3], &token).await?;
            operations.pending_send_reconciliations += 1;
            let id = fs::read_to_string(root.join("unclaimed-quote.txt"))?;
            a.fetch_mint_quote(&id, None).await?;
            ensure!(
                a.mint(&id, SplitTarget::default(), None)
                    .await?
                    .total_amount()?
                    == Amount::from(31),
                "pre-upgrade quote cannot be claimed"
            );
            let balance = a.total_balance().await?;
            let _retry = a.mint(&id, SplitTarget::default(), None).await;
            ensure!(a.total_balance().await? == balance, "quote issued twice");
            operations.completed_money_operations += 2;
            operations.quote_replay_checks += 1;
            operations.save(root)?;
            fs::write(root.join("handoff-completed"), b"complete")?;
            workload(root, &a, &b, rounds, &mut operations).await?;
        }
        "verify" => {}
        "restore" => {
            let restored_db = WalletSqliteDatabase::new(root.join("restored.sqlite")).await?;
            let restored = Wallet::new(
                &args[3],
                CurrencyUnit::Sat,
                Arc::new(restored_db),
                [1; 64],
                Some(4),
            )?;
            restored.restore().await?;
            ensure!(
                restored.total_balance().await? == a.total_balance().await?,
                "seed restore lost balance or resurrected spent proofs"
            );
            operations.seed_restores += 1;
        }
        phase => anyhow::bail!("unknown phase {phase}"),
    }
    checkpoint(root, "alice", &a).await?;
    checkpoint(root, "bob", &b).await?;
    operations.save(root)?;
    println!("{}: verified {} workload rounds", args[1], rounds);
    println!(
        "Cumulative operations: {}",
        serde_json::to_string(&operations)?
    );
    Ok(())
}
