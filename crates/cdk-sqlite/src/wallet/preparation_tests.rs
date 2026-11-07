use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cdk_common::database::wallet::{OperationPreparation, QuoteReservation};
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{
    MeltOperationData, MeltQuote, MeltSagaState, OperationData, ProofInfo, WalletSaga,
    WalletSagaState,
};
use cdk_common::{CurrencyUnit, MeltQuoteState, PaymentMethod, Proof, SecretKey, State};

use super::WalletSqliteDatabase;

#[tokio::test]
async fn preparation_crash_boundary() {
    if let Ok(directory) = std::env::var("CDK_PREPARATION_CRASH_CHILD") {
        let directory = std::path::PathBuf::from(directory);
        let request: OperationPreparation =
            serde_json::from_slice(&std::fs::read(directory.join("request.json")).unwrap())
                .unwrap();
        let db = WalletSqliteDatabase::new(directory.join("wallet.sqlite"))
            .await
            .unwrap();
        std::fs::write(directory.join("ready"), []).unwrap();
        db.prepare_operation(request).await.unwrap();
        // Exit without running Rust destructors after the commit is acknowledged.
        std::process::exit(0);
    }
    for committed in [false, true] {
        let directory = std::env::temp_dir().join(format!("cdk-crash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("wallet.sqlite");
        let db = WalletSqliteDatabase::new(path.clone()).await.unwrap();
        let mint_url = "https://mint.example.com".parse().unwrap();
        let proof = ProofInfo::new(
            Proof::new(
                2048.into(),
                "00916bbf7ef91a36".parse().unwrap(),
                "crash-test-input".parse().unwrap(),
                SecretKey::generate().public_key(),
            ),
            mint_url,
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
        let quote = MeltQuote {
            id: "crash-test-quote".to_owned(),
            mint_url: Some(proof.mint_url.clone()),
            unit: CurrencyUnit::Sat,
            amount: 1000.into(),
            fee_reserve: 10.into(),
            request: "test request".to_owned(),
            state: MeltQuoteState::Unpaid,
            expiry: 9999999999,
            payment_proof: None,
            estimated_blocks: None,
            fee_index: None,
            payment_method: PaymentMethod::BOLT11,
            used_by_operation: None,
            version: 0,
        };
        db.add_melt_quote(quote.clone()).await.unwrap();
        let saga = WalletSaga::new(
            uuid::Uuid::new_v4(),
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            quote.amount,
            proof.mint_url.clone(),
            proof.unit.clone(),
            OperationData::Melt(MeltOperationData {
                quote_id: quote.id.clone(),
                amount: quote.amount,
                fee_reserve: quote.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                metadata: HashMap::new(),
                final_proof_ys: None,
                change_blinded_messages: None,
            }),
        );
        let request = OperationPreparation {
            saga: saga.clone(),
            proof_ys: vec![proof.y],
            proofs: vec![],
            quotes: vec![QuoteReservation {
                quote_id: quote.id.clone(),
                version: 0,
            }],
            counter: None,
        };
        std::fs::write(
            directory.join("request.json"),
            serde_json::to_vec(&request).unwrap(),
        )
        .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        if !committed {
            // Keep the final insertion busy so the parent can interrupt the
            // actual preparation transaction before it reaches COMMIT.
            conn.execute_batch("CREATE TRIGGER pause_preparation BEFORE INSERT ON wallet_sagas BEGIN SELECT (WITH RECURSIVE count(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM count WHERE x < 1000000000) SELECT sum(x) FROM count); END;").unwrap();
        }
        drop(db);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "wallet::preparation_tests::preparation_crash_boundary",
                "--nocapture",
            ])
            .env("CDK_PREPARATION_CRASH_CHILD", &directory)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !directory.join("ready").exists() {
            assert!(Instant::now() < deadline, "child did not open storage");
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before preparation"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if committed {
            assert!(child.wait().unwrap().success());
        } else {
            conn.busy_timeout(Duration::ZERO).unwrap();
            loop {
                assert!(Instant::now() < deadline, "child did not begin preparation");
                match conn.execute_batch("BEGIN IMMEDIATE") {
                    Ok(()) => conn.execute_batch("ROLLBACK").unwrap(),
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::DatabaseBusy =>
                    {
                        break
                    }
                    Err(error) => panic!("unexpected lock probe: {error}"),
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            child.kill().unwrap();
            child.wait().unwrap();
        }
        let reopened = WalletSqliteDatabase::new(path).await.unwrap();
        let stored = reopened.get_proofs_by_ys(vec![proof.y]).await.unwrap();
        let stored_quote = reopened.get_melt_quote(&quote.id).await.unwrap().unwrap();
        if committed {
            assert_eq!(stored[0].state, State::Reserved);
            assert_eq!(stored[0].used_by_operation, Some(saga.id));
            assert_eq!(stored_quote.used_by_operation, Some(saga.id.to_string()));
            assert_eq!(reopened.get_saga(&saga.id).await.unwrap(), Some(saga));
        } else {
            assert_eq!(stored, vec![proof]);
            assert_eq!(stored_quote, quote);
            assert!(reopened.get_incomplete_sagas().await.unwrap().is_empty());
        }
        assert!(reopened
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .is_empty());
        drop(reopened);
        drop(conn);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
