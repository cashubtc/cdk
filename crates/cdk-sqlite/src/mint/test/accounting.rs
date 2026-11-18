//! SQLite accounting upgrade, restart and atomicity coverage.

use std::str::FromStr;

use cdk_common::database::{MintDatabase, MintSignaturesDatabase};
use cdk_common::{Amount, BlindSignature, Id, SecretKey};
use cdk_sql_common::pool::Pool;
use cdk_sql_common::stmt::{query, Column};

use super::MintSqliteDatabase;
use crate::common::{Config, SqliteConnectionManager};

#[tokio::test]
async fn accounting_upgrade_restart_and_overflow_retry() {
    let path = std::env::temp_dir().join(format!("cdk-accounting-{}.sqlite", uuid::Uuid::new_v4()));
    #[cfg(not(feature = "sqlcipher"))]
    let config: Config = (&path).into();
    #[cfg(feature = "sqlcipher")]
    let config: Config = (path.clone(), "test".to_owned()).into();
    let db = MintSqliteDatabase::new(config.clone()).await.unwrap();
    let pool = Pool::<SqliteConnectionManager>::new(config.clone());
    let conn = pool.get().await.unwrap();
    query(
        "DROP VIEW keyset_accounting_totals; DROP TABLE keyset_accounting;
        INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
        VALUES ('00916bbf7ef91a36', 1099511627776, 100, 3)",
    )
    .unwrap()
    .batch(&*conn)
    .await
    .unwrap();
    query(include_str!("../../../../cdk-sql-common/src/mint/migrations/sqlite/20261118000000_add_keyset_accounting.sql"))
        .unwrap().batch(&*conn).await.unwrap();
    let id = Id::from_str("00916bbf7ef91a36").unwrap();
    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_blind_signatures(
        &[SecretKey::generate().public_key()],
        &[BlindSignature {
            amount: Amount::from(128),
            keyset_id: id,
            c: SecretKey::generate().public_key(),
            dleq: None,
        }],
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    db.close().await.unwrap();
    let db = MintSqliteDatabase::new(config).await.unwrap();
    assert_eq!(
        db.get_total_issued().await.unwrap()[&id],
        Amount::from((1_u64 << 40) + 128)
    );
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 1);
    assert_eq!(
        query("SELECT fee_collected FROM keyset_accounting_totals")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(3))
    );

    query("INSERT INTO keyset_amounts (keyset_id, total_issued) VALUES ('a', 10), ('z', 9223372036854775807);
        INSERT INTO keyset_accounting (event_id, keyset_id, total_issued) VALUES ('a', 'a', 1), ('z', 'z', 1)")
        .unwrap().batch(&*conn).await.unwrap();
    assert!(db.flush_keyset_accounting(10).await.is_err());
    // A write waits for the cancelled aggregation transaction's rollback.
    query("UPDATE keyset_amounts SET total_issued = 0 WHERE keyset_id = 'z'")
        .unwrap()
        .execute(&*conn)
        .await
        .unwrap();
    assert_eq!(
        query("SELECT total_issued FROM keyset_amounts WHERE keyset_id = 'a'")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(10))
    );
    assert_eq!(
        query("SELECT COUNT(*) FROM keyset_accounting")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(2))
    );
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 2);
    assert!(db.flush_keyset_accounting(0).await.is_err());
    db.close().await.unwrap();
    drop(conn);
    pool.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn accounting_batch_exceeds_sqlite_parameter_limit() {
    const EVENTS: usize = 32_767;

    let path = std::env::temp_dir().join(format!(
        "cdk-accounting-large-batch-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    #[cfg(not(feature = "sqlcipher"))]
    let config: Config = (&path).into();
    #[cfg(feature = "sqlcipher")]
    let config: Config = (path.clone(), "test".to_owned()).into();
    let db = MintSqliteDatabase::new(config.clone()).await.unwrap();
    let pool = Pool::<SqliteConnectionManager>::new(config);
    let conn = pool.get().await.unwrap();
    query(
        "WITH RECURSIVE events(n) AS (
        SELECT 1 UNION ALL SELECT n + 1 FROM events WHERE n < :events
    ) INSERT INTO keyset_accounting (event_id, keyset_id, total_issued)
      SELECT CAST(n AS TEXT), '00916bbf7ef91a36', 1 FROM events",
    )
    .unwrap()
    .bind("events", i64::try_from(EVENTS).unwrap())
    .execute(&*conn)
    .await
    .unwrap();

    // Fail late in the ordered event batch, after earlier deletes can succeed.
    query(
        "CREATE TRIGGER reject_accounting_delete BEFORE DELETE ON keyset_accounting
        WHEN OLD.event_id = '999' BEGIN SELECT RAISE(ABORT, 'accounting deletion failed'); END",
    )
    .unwrap()
    .batch(&*conn)
    .await
    .unwrap();
    let error = db
        .flush_keyset_accounting(EVENTS)
        .await
        .expect_err("deletion must fail");
    assert!(error.to_string().contains("accounting deletion failed"));
    // This write waits for rollback of every earlier delete and totals update.
    query("DROP TRIGGER reject_accounting_delete")
        .unwrap()
        .execute(&*conn)
        .await
        .unwrap();
    assert_eq!(
        query("SELECT COUNT(*) FROM keyset_accounting")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(EVENTS as i64))
    );
    assert_eq!(
        query("SELECT COUNT(*) FROM keyset_amounts")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(0))
    );

    assert_eq!(db.flush_keyset_accounting(EVENTS).await.unwrap(), EVENTS);
    assert_eq!(
        query("SELECT COUNT(*) FROM keyset_accounting")
            .unwrap()
            .pluck(&*conn)
            .await
            .unwrap(),
        Some(Column::Integer(0))
    );
    assert_eq!(
        db.get_total_issued().await.unwrap()[&Id::from_str("00916bbf7ef91a36").unwrap()],
        Amount::from(EVENTS as u64)
    );
    assert_eq!(db.flush_keyset_accounting(EVENTS).await.unwrap(), 0);

    db.close().await.unwrap();
    drop(conn);
    pool.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
