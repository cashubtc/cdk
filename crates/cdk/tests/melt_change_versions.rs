//! Versioned mint regression: paid settlement must not discard late change.
#![cfg(feature = "wallet")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk::nuts::{CurrencyUnit, MeltQuoteState, PaymentMethod};
use cdk::wallet::{MeltOutcome, Wallet};
use cdk::Amount;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{MeltSagaState, TransactionDirection, TransactionStatus, WalletSagaState};
use cdk_sqlite::WalletSqliteDatabase;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use serde_json::{json, Value};

struct Proxy(Child);
impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn control(url: &str, enabled: bool) {
    ureq::post(&format!("{url}/test/faults"))
        .send_json(json!({"enabled": enabled}))
        .expect("fault configuration request must succeed");
}

/// Run with the official 0.17.7 and 0.18.1 fakewallet mints via the matrix runner.
#[tokio::test]
#[ignore = "requires versioned local mints; run misc/test-melt-change-versions.py"]
async fn paid_melt_collects_late_change_after_sqlite_reopen_without_resending() {
    let mints = std::env::var("CDK_MELT_CHANGE_MINTS").expect("matrix mint URLs");
    for upstream in mints.split(',') {
        let mut child = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/melt_change_proxy.py"
            ))
            .arg(upstream)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut port = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut port)
            .unwrap();
        let _proxy = Proxy(child);
        let url = format!("http://127.0.0.1:{}", port.trim());
        let path =
            std::env::temp_dir().join(format!("melt-change-{}.sqlite", uuid::Uuid::new_v4()));
        let seed = rand::random::<[u8; 64]>();
        let db = Arc::new(WalletSqliteDatabase::new(path.clone()).await.unwrap());
        let wallet = Wallet::new(&url, CurrencyUnit::Sat, db.clone(), seed, None).unwrap();
        let quote = wallet
            .mint_quote(PaymentMethod::BOLT11, Some(Amount::from(100)), None, None)
            .await
            .unwrap();
        wallet
            .wait_and_mint_quote(
                quote,
                Default::default(),
                Default::default(),
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        let key = SecretKey::from_slice(&[42; 32]).unwrap();
        let invoice = InvoiceBuilder::new(Currency::Bitcoin)
            .description("late-change regression".to_owned())
            .payment_hash(sha256::Hash::hash(&rand::random::<[u8; 32]>()))
            .payment_secret(PaymentSecret([21; 32]))
            .amount_milli_satoshis(21_000)
            .current_timestamp()
            .min_final_cltv_expiry_delta(144)
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &key))
            .unwrap()
            .to_string();
        let quote = wallet
            .melt_quote(PaymentMethod::BOLT11, invoice, None, None)
            .await
            .unwrap();
        assert_eq!(
            quote.fee_reserve,
            Amount::from(10),
            "fixture must leave change inside the reserve"
        );
        control(&url, true);
        let prepared = wallet
            .prepare_melt(&quote.id, Default::default())
            .await
            .unwrap();
        let operation = prepared.operation_id();
        let outcome =
            tokio::time::timeout(Duration::from_secs(15), prepared.confirm_prefer_async())
                .await
                .unwrap()
                .unwrap();
        match outcome {
            MeltOutcome::Paid(paid) => assert_eq!(paid.state(), MeltQuoteState::Paid),
            MeltOutcome::Pending(_) => panic!("the mint has paid this invoice"),
        }
        let saga = db
            .get_saga(&operation)
            .await
            .unwrap()
            .expect("late change must retain its recovery record");
        assert_eq!(
            saga.state,
            WalletSagaState::Melt(MeltSagaState::ChangePending)
        );
        assert!(db
            .get_reserved_proofs(&operation)
            .await
            .unwrap()
            .iter()
            .all(|proof| proof.state == cdk::nuts::State::Spent));
        // Open a new SQLite connection and wallet with the same seed.
        let reopened_db = Arc::new(WalletSqliteDatabase::new(path.clone()).await.unwrap());
        let reopened =
            Wallet::new(&url, CurrencyUnit::Sat, reopened_db.clone(), seed, None).unwrap();
        assert_eq!(
            reopened.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        control(&url, false);
        assert_eq!(
            reopened.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(reopened.total_balance().await.unwrap(), Amount::from(78));
        assert_eq!(
            reopened.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        let txs = reopened
            .list_transactions(Some(TransactionDirection::Outgoing))
            .await
            .unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].status, TransactionStatus::Completed);
        assert_eq!(txs[0].fee, Amount::from(1));
        let requests: Vec<Value> = ureq::get(&format!("{url}/test/requests"))
            .call()
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "POST"
                    && r["path"] == "/v1/melt/bolt11"
                    && r["forwarded"] == true)
                .count(),
            1
        );
        assert!(reopened_db.get_saga(&operation).await.unwrap().is_none());
        println!("passed late-change recovery against {upstream}");
        drop(reopened);
        drop(reopened_db);
        drop(wallet);
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
