//! PostgreSQL accounting coordination and upgrade tests.

#![allow(clippy::unwrap_used)]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::{MintDatabase, MintSignaturesDatabase};
use cdk_common::{Amount, BlindSignature, Id, SecretKey};
use tokio_postgres::{Client, NoTls};

use crate::MintPgDatabase;

async fn setup() -> (Arc<MintPgDatabase>, Client, String) {
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .unwrap_or_else(|_| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        });
    let schema = format!("accounting_{}", uuid::Uuid::new_v4().simple());
    let config = format!("{url} application_name={schema} schema={schema}");
    let db = MintPgDatabase::new(config.as_str()).await.unwrap();
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(connection);
    client
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    (Arc::new(db), client, config)
}

fn signature() -> (cdk_common::PublicKey, BlindSignature) {
    (
        SecretKey::generate().public_key(),
        BlindSignature {
            amount: Amount::from(128),
            keyset_id: Id::from_str("00916bbf7ef91a36").unwrap(),
            c: SecretKey::generate().public_key(),
            dleq: None,
        },
    )
}

async fn issue(db: &MintPgDatabase) {
    let (message, signature) = signature();
    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_blind_signatures(&[message], &[signature], None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn count(client: &Client, table: &str) -> i64 {
    client
        .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn operations_proceed_while_keyset_totals_are_locked() {
    let (db, client, _) = setup().await;
    issue(&db).await;
    db.flush_keyset_accounting(10).await.unwrap();
    client
        .batch_execute("BEGIN; SELECT * FROM keyset_amounts FOR UPDATE")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), issue(&db))
        .await
        .expect("issuance must not lock shared totals");
    assert_eq!(count(&client, "keyset_accounting").await, 1);
    client.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
}

#[tokio::test]
async fn workers_skip_claimed_events_and_preserve_live_totals() {
    let (db, client, _) = setup().await;
    for _ in 0..3 {
        issue(&db).await;
    }
    client
        .batch_execute(
            "BEGIN; SELECT * FROM keyset_accounting ORDER BY event_id LIMIT 1 FOR UPDATE",
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), db.flush_keyset_accounting(10))
            .await
            .unwrap()
            .unwrap(),
        2
    );
    assert_eq!(count(&client, "keyset_accounting").await, 1);
    let id = signature().1.keyset_id;
    assert_eq!(db.get_total_issued().await.unwrap()[&id], Amount::from(384));
    client.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
    assert_eq!(db.get_total_issued().await.unwrap()[&id], Amount::from(384));
}

#[tokio::test]
async fn concurrent_workers_apply_each_event_once() {
    let (db, client, _) = setup().await;
    for _ in 0..24 {
        issue(&db).await;
    }
    let (first, second) = tokio::join!(
        async {
            let mut total = 0;
            loop {
                let n = db.flush_keyset_accounting(3).await.unwrap();
                total += n;
                if n == 0 {
                    break;
                }
            }
            total
        },
        async {
            let mut total = 0;
            loop {
                let n = db.flush_keyset_accounting(3).await.unwrap();
                total += n;
                if n == 0 {
                    break;
                }
            }
            total
        }
    );
    assert_eq!(first + second, 24);
    assert_eq!(count(&client, "keyset_accounting").await, 0);
    assert_eq!(
        db.get_total_issued().await.unwrap()[&signature().1.keyset_id],
        Amount::from(24 * 128)
    );
}

#[tokio::test]
async fn late_commit_is_not_lost_after_a_flush() {
    let (db, _, _) = setup().await;
    let mut late = db.begin_transaction().await.unwrap();
    let (message, sig) = signature();
    late.add_blind_signatures(&[message], &[sig], None)
        .await
        .unwrap();
    issue(&db).await;
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
    late.commit().await.unwrap();
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
    assert_eq!(
        db.get_total_issued().await.unwrap()[&signature().1.keyset_id],
        Amount::from(256)
    );
}

#[tokio::test]
async fn failed_batch_rolls_back_totals_and_retains_events() {
    let (db, client, _) = setup().await;
    client.batch_execute("INSERT INTO keyset_amounts (keyset_id, total_issued) VALUES ('a', 10), ('z', 9223372036854775807);
        INSERT INTO keyset_accounting (event_id, keyset_id, total_issued) VALUES ('a', 'a', 1), ('z', 'z', 1)").await.unwrap();
    assert!(db.flush_keyset_accounting(10).await.is_err());
    // Wait for the dropped transaction's rollback by obtaining its row locks.
    client
        .batch_execute("BEGIN; SELECT * FROM keyset_amounts ORDER BY keyset_id FOR UPDATE")
        .await
        .unwrap();
    assert_eq!(count(&client, "keyset_accounting").await, 2);
    let issued: i64 = client
        .query_one(
            "SELECT total_issued FROM keyset_amounts WHERE keyset_id = 'a'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(issued, 10);
    client
        .batch_execute("UPDATE keyset_amounts SET total_issued = 0 WHERE keyset_id = 'z'; COMMIT")
        .await
        .unwrap();
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 2);
    assert_eq!(count(&client, "keyset_accounting").await, 0);
}

#[tokio::test]
async fn cancelled_flush_releases_claims_for_retry() {
    let (db, client, _) = setup().await;
    issue(&db).await;
    db.flush_keyset_accounting(10).await.unwrap();
    issue(&db).await;
    client
        .batch_execute("BEGIN; SELECT * FROM keyset_amounts FOR UPDATE")
        .await
        .unwrap();
    // The event can be claimed, but applying it cannot finish until the totals unlock.
    let flush = tokio::spawn({
        let db = db.clone();
        async move { db.flush_keyset_accounting(10).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: i64 = client.query_one("SELECT COUNT(*) FROM pg_stat_activity WHERE application_name = current_schema() AND wait_event_type = 'Lock' AND query LIKE '%INSERT INTO keyset_amounts%'", &[]).await.unwrap().get(0);
            if waiting > 0 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("flush must be blocked applying its claimed events");
    flush.abort();
    assert!(flush.await.unwrap_err().is_cancelled());
    client.batch_execute("ROLLBACK").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if db.flush_keyset_accounting(10).await.unwrap() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled batch must remain available");
    assert_eq!(
        db.get_total_issued().await.unwrap()[&signature().1.keyset_id],
        Amount::from(256)
    );
}

#[tokio::test]
async fn upgrade_preserves_populated_totals_and_restart_keeps_journal() {
    let (db, client, config) = setup().await;
    client
        .batch_execute(
            "DROP VIEW keyset_accounting_totals; DROP TABLE keyset_accounting;
        INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
        VALUES ('00916bbf7ef91a36', 1099511627776, 100, 3)",
        )
        .await
        .unwrap();
    client.batch_execute(include_str!("../../cdk-sql-common/src/mint/migrations/postgres/20261118000000_add_keyset_accounting.sql")).await.unwrap();
    issue(&db).await;
    db.close().await.unwrap();
    let reopened = MintPgDatabase::new(config.as_str()).await.unwrap();
    let id = signature().1.keyset_id;
    assert_eq!(
        reopened.get_total_issued().await.unwrap()[&id],
        Amount::from((1_u64 << 40) + 128)
    );
    assert_eq!(reopened.flush_keyset_accounting(10).await.unwrap(), 1);
    let row = client
        .query_one(
            "SELECT total_issued, total_redeemed, fee_collected FROM keyset_accounting_totals",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), (1_i64 << 40) + 128);
    assert_eq!(row.get::<_, i64>(1), 100);
    assert_eq!(row.get::<_, i64>(2), 3);
}

#[tokio::test]
async fn completed_operation_fees_are_live_before_and_after_aggregation() {
    use std::collections::HashMap;

    use cdk_common::mint::Operation;

    let (db, client, _) = setup().await;
    let id = signature().1.keyset_id;
    let operation = Operation::new_swap(Amount::from(8), Amount::from(9), Amount::from(1));
    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_completed_operation(&operation, &HashMap::from([(id, Amount::from(1))]))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&client, "keyset_amounts").await, 0);
    let fees: i64 = client
        .query_one("SELECT fee_collected FROM keyset_accounting_totals", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(fees, 1);
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
    let fees: i64 = client
        .query_one("SELECT fee_collected FROM keyset_accounting_totals", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(fees, 1);
}
