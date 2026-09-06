use std::sync::Arc;

use cdk_sql_common::database::SqlBackend;
use cdk_sql_common::stmt::query;
use cdk_sql_common::value::Value;
use futures_util::poll;

use super::*;
use crate::{PgConfig, PostgresBackend};

fn url() -> String {
    std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .unwrap_or_else(|_| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        })
}

fn backend(schema: Option<String>, capacity: usize) -> PostgresBackend {
    let mut config = PgConfig::new(&url(), None, Some(capacity), Some(10));
    config.schema = schema;
    PostgresBackend::new(config).unwrap()
}

async fn database() -> PostgresBackend {
    let backend = backend(Some(format!("pool_{}", uuid::Uuid::new_v4().simple())), 1);
    let tx = backend.begin_migration().await.unwrap();
    tx.batch(query("CREATE TABLE test (id BIGINT PRIMARY KEY)").unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    backend
}

async fn count(backend: &PostgresBackend) -> i64 {
    match backend
        .acquire()
        .await
        .unwrap()
        .pluck(query("SELECT COUNT(*) FROM test").unwrap())
        .await
        .unwrap()
    {
        Some(Value::Integer(n)) => n,
        other => panic!("unexpected count: {other:?}"),
    }
}

#[test]
fn reject_zero_capacity_and_invalid_tls_without_exposing_credentials() {
    assert!(PostgresBackend::new(PgConfig::new(&url(), None, Some(0), None)).is_err());
    assert!(PostgresBackend::new(PgConfig::new(
        "host=localhost password=secret",
        Some("invalid"),
        None,
        None
    ))
    .is_err());
    let config = PgConfig::new(
        "postgres://user:secret@localhost/db?sslmode=verify-full",
        Some("disable"),
        None,
        None,
    );
    let (driver, tls) = config.driver_config().unwrap();
    assert_eq!(
        driver.get_ssl_mode(),
        tokio_postgres::config::SslMode::Disable
    );
    assert!(tls.is_none());
    assert!(!format!("{config:?}").contains("secret"));
    let invalid = PostgresBackend::new(PgConfig::from("secret=value")).unwrap_err();
    assert!(!format!("{invalid:?}").contains("secret"));
    assert!(!invalid.to_string().contains("secret"));
}

#[tokio::test]
async fn failed_initial_connection_is_reported_by_constructor() {
    let config = PgConfig::new(
        "host=127.0.0.1 port=1 user=invalid dbname=invalid",
        None,
        Some(1),
        Some(1),
    );
    let backend = PostgresBackend::new(config.clone()).unwrap();
    for _ in 0..2 {
        assert!(backend.acquire().await.is_err());
        assert_eq!(backend.pool.status().size, 0);
    }
    #[cfg(feature = "mint")]
    assert!(crate::MintPgDatabase::new(config).await.is_err());
}

#[tokio::test]
async fn commit_rollback_drop_and_query_error_are_isolated() {
    let backend = database().await;
    for finish in 0_i64..4 {
        let tx = backend.begin_transaction().await.unwrap();
        tx.execute(
            query("INSERT INTO test VALUES (:id)")
                .unwrap()
                .bind("id", finish),
        )
        .await
        .unwrap();
        match finish {
            0 => tx.commit().await.unwrap(),
            1 => tx.rollback().await.unwrap(),
            2 => drop(tx),
            _ => {
                assert!(tx
                    .execute(query("INSERT INTO test VALUES (0)").unwrap())
                    .await
                    .is_err());
                tx.rollback().await.unwrap();
            }
        }
        assert_eq!(count(&backend).await, 1);
    }
}

#[tokio::test]
async fn cancelled_begin_commit_rollback_never_release_an_open_transaction() {
    let backend = database().await;
    let conn = backend.acquire().await.unwrap();
    let mut begin = Box::pin(conn.begin_transaction());
    assert!(poll!(&mut begin).is_pending());
    drop(begin);
    assert_eq!(count(&backend).await, 0);
    for commit in [false, true] {
        let tx = backend.begin_transaction().await.unwrap();
        tx.execute(query("INSERT INTO test VALUES (1)").unwrap())
            .await
            .unwrap();
        let mut finish = if commit { tx.commit() } else { tx.rollback() };
        assert!(poll!(&mut finish).is_pending());
        drop(finish);
        // A cancelled COMMIT has an uncertain outcome. Either is acceptable;
        // the next borrower must always see a finalized transaction.
        let conn = backend.acquire().await.unwrap();
        let raw = conn.lease.lock().await;
        let messages = raw.as_ref().unwrap().client().simple_query(
            "SELECT xact_start = query_start FROM pg_stat_activity WHERE pid = pg_backend_pid()",
        ).await.unwrap();
        let row = messages
            .iter()
            .find_map(|message| match message {
                tokio_postgres::SimpleQueryMessage::Row(row) => Some(row),
                _ => None,
            })
            .expect("expected row");
        assert_eq!(
            row.get(0),
            Some("t"),
            "next borrower inherited an open transaction"
        );
        drop(raw);
        let n = count_from(&conn).await;
        assert!(n == 0 || (commit && n == 1));
    }
}

async fn count_from(conn: &PostgresConnection) -> i64 {
    match conn
        .pluck(query("SELECT COUNT(*) FROM test").unwrap())
        .await
        .unwrap()
    {
        Some(Value::Integer(n)) => n,
        other => panic!("unexpected count: {other:?}"),
    }
}

#[tokio::test]
async fn exhausted_and_cancelled_acquisitions_preserve_capacity() {
    let backend = database().await;
    let held = backend.acquire().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), backend.acquire())
            .await
            .is_err()
    );
    let timeouts = deadpool_postgres::Timeouts {
        wait: Some(Duration::from_millis(10)),
        ..Default::default()
    };
    assert!(matches!(
        backend.pool.timeout_get(&timeouts).await,
        Err(deadpool_postgres::PoolError::Timeout(
            deadpool_postgres::TimeoutType::Wait
        ))
    ));
    drop(held);
    assert_eq!(count(&backend).await, 0);
}

#[tokio::test]
#[ignore = "requires isolated fault-test invocation; run by misc/pgbouncer/test.sh"]
async fn cancelled_blocked_query_keeps_checkout_until_rollback() {
    cancelled_blocked_query(false).await;
}

#[tokio::test]
#[ignore = "requires isolated fault-test invocation; run by misc/pgbouncer/test.sh"]
async fn cleanup_timeout_discards_connection_before_recovery() {
    cancelled_blocked_query(true).await;
}

async fn cancelled_blocked_query(expire_cleanup: bool) {
    #[cfg(feature = "prometheus")]
    let timeouts = cleanup_count("timeout");
    let backend = database().await;
    let control_backend = backend_without_schema();
    let control = control_backend.pool.get().await.unwrap();
    let key = uuid::Uuid::new_v4().as_u128() as i64;
    control.batch_execute("BEGIN").await.unwrap();
    control
        .query_one("SELECT pg_advisory_xact_lock($1)", &[&key])
        .await
        .unwrap();
    let tx = Arc::new(backend.begin_transaction().await.unwrap());
    if expire_cleanup {
        tx.0.lease.lock().await.as_mut().unwrap().timeout = Duration::from_millis(100);
    }
    tx.execute(query("INSERT INTO test VALUES (1)").unwrap())
        .await
        .unwrap();
    let task = tokio::spawn({
        let tx = tx.clone();
        async move {
            tx.0.run(move |c| {
                Box::pin(async move {
                    c.query_one("SELECT pg_advisory_xact_lock($1)", &[&key])
                        .await
                        .map_err(database_error)?;
                    Ok(())
                })
            })
            .await
        }
    });
    // Observe the actual lock wait, rather than assuming the query was sent.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = control.query_one("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND objid=($1::bigint & 4294967295)::oid)", &[&key]).await.unwrap().get(0);
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), backend.acquire())
            .await
            .is_err()
    );
    if expire_cleanup {
        tokio::time::timeout(Duration::from_secs(2), async {
            while backend.pool.status().size != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup must remove the object by its deadline");
        #[cfg(feature = "prometheus")]
        assert_eq!(cleanup_count("timeout"), timeouts + 1.0);
    }
    control.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(count(&backend).await, 0);
    assert!(tx
        .execute(query("INSERT INTO test VALUES (2)").unwrap())
        .await
        .is_err());
}

fn backend_without_schema() -> PostgresBackend {
    backend(None, 2)
}

#[tokio::test]
#[ignore = "requires isolated fault-test invocation; run by misc/pgbouncer/test.sh"]
async fn terminated_transaction_and_idle_connection_recover_without_replaying_writes() {
    #[cfg(feature = "prometheus")]
    let failures = cleanup_count("rollback_error");
    let backend = database().await;
    let control_backend = backend_without_schema();
    let control = control_backend.pool.get().await.unwrap();
    let tx = backend.begin_transaction().await.unwrap();
    tx.execute(query("INSERT INTO test VALUES (1)").unwrap())
        .await
        .unwrap();
    let pid = tx
        .pluck(query("SELECT pg_backend_pid()").unwrap())
        .await
        .unwrap()
        .unwrap();
    let Value::Integer(pid) = pid else {
        panic!("invalid PID")
    };
    control
        .query_one("SELECT pg_terminate_backend($1)", &[&(pid as i32)])
        .await
        .unwrap();
    assert!(tx.commit().await.is_err());
    assert_eq!(count(&backend).await, 0);
    #[cfg(feature = "prometheus")]
    assert_eq!(cleanup_count("rollback_error"), failures + 1.0);
    // Session/direct endpoints retain their server when idle. Transaction mode
    // releases it and the proxy itself reconnects it on the next transaction.
    let object = backend.pool.get().await.unwrap();
    object.batch_execute("BEGIN").await.unwrap();
    let pid: i32 = object
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    control
        .query_one("SELECT pg_terminate_backend($1)", &[&pid])
        .await
        .unwrap();
    drop(object);
    assert_eq!(count(&backend).await, 0);
}

#[tokio::test]
async fn schema_isolation_includes_quoted_identifiers_and_failed_bootstrap() {
    let a = backend(Some(format!("a,\"{}", uuid::Uuid::new_v4().simple())), 2);
    let b = backend(Some(format!("b_{}", uuid::Uuid::new_v4().simple())), 2);
    for (backend, value) in [(&a, 11_i64), (&b, 22_i64)] {
        let tx = backend.begin_migration().await.unwrap();
        tx.batch(query("CREATE TABLE test (id BIGINT PRIMARY KEY)").unwrap())
            .await
            .unwrap();
        assert!(tx.batch(query("SELECT 1 / 0").unwrap()).await.is_err());
        drop(tx); // Failed bootstrap must roll back its DDL before the next startup.
        let tx = backend.begin_migration().await.unwrap();
        tx.batch(query("CREATE TABLE test (id BIGINT PRIMARY KEY)").unwrap())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let conn = backend.acquire().await.unwrap();
        conn.execute(
            query("INSERT INTO test VALUES (:id)")
                .unwrap()
                .bind("id", value),
        )
        .await
        .unwrap();
    }
    for _ in 0..6 {
        for (backend, value) in [(&a, 11_i64), (&b, 22_i64)] {
            let conn = backend.acquire().await.unwrap();
            assert!(conn
                .execute(query("INSERT INTO test SELECT id FROM test").unwrap())
                .await
                .is_err());
            conn.batch(query("UPDATE test SET id = id; SELECT id FROM test").unwrap())
                .await
                .unwrap();
            assert_eq!(
                conn.pluck(query("SELECT id FROM test").unwrap())
                    .await
                    .unwrap(),
                Some(Value::Integer(value))
            );
            drop(conn);
            let tx = backend.begin_transaction().await.unwrap();
            assert_eq!(
                tx.pluck(query("SELECT id FROM test").unwrap())
                    .await
                    .unwrap(),
                Some(Value::Integer(value))
            );
            tx.commit().await.unwrap();
        }
    }
}

#[cfg(feature = "mint")]
#[tokio::test]
async fn concurrent_startup_serializes_schema_and_migrations() {
    let schema = format!("startup_{}", uuid::Uuid::new_v4().simple());
    let config = PgConfig::new(&format!("{} schema={schema}", url()), None, Some(1), None);
    let (a, b) = tokio::join!(
        crate::MintPgDatabase::new(config.clone()),
        crate::MintPgDatabase::new(config)
    );
    assert!(a.is_ok(), "{a:?}");
    assert!(b.is_ok(), "{b:?}");
}

// Run separately and serially: it deliberately occupies both proxy backends.
#[tokio::test]
#[ignore = "requires isolated PgBouncer transaction endpoint; run by misc/pgbouncer/test.sh"]
async fn pgbouncer_prepared_statement_survives_verified_backend_switch() {
    assert_eq!(
        std::env::var("CDK_TEST_PGBOUNCER_MODE").as_deref(),
        Ok("transaction")
    );
    let a = backend_without_schema();
    let b = backend_without_schema();
    let client = a.pool.get().await.unwrap();
    let other = b.pool.get().await.unwrap();
    let prepared = client
        .prepare("SELECT pg_backend_pid(), $1::bigint")
        .await
        .unwrap();
    let initial: i32 = client.query_one(&prepared, &[&7_i64]).await.unwrap().get(0);
    // Pin the original backend to another client, forcing the same prepared
    // statement on client A to execute on a different PostgreSQL process.
    let mut pinned = false;
    for _ in 0..20 {
        other.batch_execute("BEGIN").await.unwrap();
        let pid: i32 = other
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        if pid == initial {
            pinned = true;
            break;
        }
        other.batch_execute("ROLLBACK").await.unwrap();
    }
    assert!(pinned, "could not pin the original backend");
    let switched = client.query_one(&prepared, &[&42_i64]).await.unwrap();
    assert_ne!(
        initial,
        switched.get::<_, i32>(0),
        "test must demonstrate an actual backend switch"
    );
    assert_eq!(switched.get::<_, i64>(1), 42);
    other.batch_execute("ROLLBACK").await.unwrap();
    drop(other);
    drop(client);
    // More local client connections than server connections remain usable.
    let mut clients = Vec::new();
    for _ in 0..8 {
        clients.push(backend(None, 1));
    }
    let mut connections = Vec::new();
    for backend in &clients {
        connections.push(backend.acquire().await.unwrap());
    }
    for conn in connections {
        assert_eq!(
            conn.pluck(query("SELECT 42::bigint").unwrap())
                .await
                .unwrap(),
            Some(Value::Integer(42))
        );
    }
}

#[test]
fn runtime_shutdown_disposes_unfinished_transaction() {
    for queued_cleanup in [false, true] {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (backend, tx) = runtime.block_on(async {
            let backend = database().await;
            let tx = backend.begin_transaction().await.unwrap();
            (backend, tx)
        });
        if queued_cleanup {
            runtime.block_on(async {
                drop(tx);
            });
            drop(runtime);
        } else {
            drop(runtime);
            drop(tx);
        }
        assert_eq!(backend.pool.status().size, 0);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(runtime.block_on(count(&backend)), 0);
    }
}

#[cfg(feature = "mint")]
#[tokio::test]
async fn auth_proof_spend_is_atomic_across_two_pools() {
    use std::str::FromStr;

    use cdk_common::database::MintAuthDatabase;
    use cdk_common::secret::Secret;
    use cdk_common::{AuthProof, Id, SecretKey, State};

    let schema = format!("auth_{}", uuid::Uuid::new_v4().simple());
    let config = PgConfig::new(&format!("{} schema={schema}", url()), None, Some(1), None);
    let a = crate::MintPgAuthDatabase::new(config.clone())
        .await
        .unwrap();
    let b = crate::MintPgAuthDatabase::new(config).await.unwrap();
    let proof = AuthProof {
        keyset_id: Id::from_str("009a1f293253e41e").unwrap(),
        secret: Secret::new("pool-auth-test"),
        c: SecretKey::generate().public_key(),
        dleq: None,
    };
    let spend = |db: crate::MintPgAuthDatabase, proof: AuthProof| async move {
        let mut tx = MintAuthDatabase::begin_transaction(&db).await.unwrap();
        match tx.add_proof(proof).await {
            Ok(()) => {
                tx.commit().await.unwrap();
                true
            }
            Err(Error::Duplicate) => {
                tx.rollback().await.unwrap();
                false
            }
            Err(error) => panic!("unexpected auth failure: {error}"),
        }
    };
    let (first, second) = tokio::join!(spend(a.clone(), proof.clone()), spend(b, proof.clone()));
    assert_ne!(first, second);
    assert_eq!(
        a.get_proofs_states(&[proof.y().unwrap()]).await.unwrap(),
        vec![Some(State::Spent)]
    );
}

#[tokio::test]
#[ignore = "disconnects all clients on the isolated test proxy; run serially"]
async fn pgbouncer_proxy_disconnect_recovers_without_duplicate_writes() {
    assert_eq!(
        std::env::var("CDK_TEST_PGBOUNCER_MODE").as_deref(),
        Ok("transaction")
    );
    let backend = database().await;
    let tx = backend.begin_transaction().await.unwrap();
    tx.execute(query("INSERT INTO test VALUES (1)").unwrap())
        .await
        .unwrap();
    let (mut config, _) = PgConfig::from(url().as_str()).driver_config().unwrap();
    let db = config.get_dbname().unwrap().replace('"', "\"\"");
    config.dbname("pgbouncer");
    let (admin, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    let driver = tokio::spawn(connection);
    admin.simple_query(&format!("KILL \"{db}\"")).await.unwrap();
    admin
        .simple_query(&format!("RESUME \"{db}\""))
        .await
        .unwrap();
    assert!(tx.commit().await.is_err());
    assert_eq!(count(&backend).await, 0);
    let tx = backend.begin_transaction().await.unwrap();
    tx.execute(query("INSERT INTO test VALUES (2)").unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&backend).await, 1);
    drop(admin);
    driver.abort();
}

#[cfg(feature = "wallet")]
#[tokio::test]
async fn derivation_reservations_are_atomic_across_independent_pools() {
    use cdk_common::database::WalletDatabase;

    let schema = format!("counter_{}", uuid::Uuid::new_v4().simple());
    let config = PgConfig::from(format!("{} schema={schema}", url()).as_str());
    let first = crate::WalletPgDatabase::new(config.clone()).await.unwrap();
    let second = crate::WalletPgDatabase::new(config).await.unwrap();
    let (a, b, c, d) = tokio::join!(
        first.reserve_derivation_index("p2pk", 6),
        second.reserve_derivation_index("p2pk", 6),
        first.reserve_derivation_index("p2pk", 6),
        second.reserve_derivation_index("p2pk", 6),
    );
    let mut indexes = [a, b, c, d].map(Result::unwrap);
    indexes.sort_unstable();
    assert_eq!(indexes, [6, 7, 8, 9]);
}

#[cfg(feature = "prometheus")]
fn cleanup_count(reason: &str) -> f64 {
    cdk_prometheus::METRICS
        .registry()
        .gather()
        .iter()
        .filter(|family| family.name() == "cdk_db_connections_discarded_total")
        .flat_map(|family| family.get_metric())
        .filter(|metric| {
            metric
                .get_label()
                .iter()
                .any(|label| label.name() == "reason" && label.value() == reason)
        })
        .map(|metric| metric.get_counter().value())
        .sum()
}
