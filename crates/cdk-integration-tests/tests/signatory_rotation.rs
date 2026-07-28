//! Cross-instance keyset rotation against a shared Postgres database.
//!
//! Two `DbSignatory` instances, each with its own process-local rotation
//! lock, drive concurrent rotations of the same unit through separate
//! Postgres connection pools pointing at one schema. Nothing in-process
//! serializes them: the only guard is `pg_advisory_xact_lock(hashtext(unit))`
//! inside `next_derivation_index`. The test asserts every rotation still gets
//! a unique keyset id and path index.
//!
//! Requires Postgres. Set `CDK_MINTD_DATABASE_URL` (or `PG_DB_URL`) to a
//! reachable server (see `docker-compose.postgres.yaml`). With neither set the
//! test prints a skip notice and passes, so a plain `cargo test` without
//! Postgres does not fail.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cdk_common::database::{Error, MintKeysDatabase};
use cdk_common::nut02::KeySetVersion;
use cdk_common::nuts::CurrencyUnit;
use cdk_postgres::MintPgDatabase;
use cdk_signatory::db_signatory::DbSignatory;
use cdk_signatory::signatory::{RotateKeyArguments, Signatory};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_rotations_across_pg_instances_do_not_collide() {
    let Some(base_url) = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .ok()
    else {
        eprintln!(
            "skipping concurrent_rotations_across_pg_instances_do_not_collide: \
             set CDK_MINTD_DATABASE_URL (or PG_DB_URL) to run"
        );
        return;
    };

    // Isolate this run in its own schema, matching the cdk-postgres conformance
    // harness. Both pools use the same schema so they share the keyset table.
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let schema = format!(
        "rot_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let url = format!("{base_url} schema={schema}");

    let store_a: Arc<dyn MintKeysDatabase<Err = Error> + Send + Sync> =
        Arc::new(MintPgDatabase::new(url.as_str()).await.expect("pg db a"));
    let store_b: Arc<dyn MintKeysDatabase<Err = Error> + Send + Sync> =
        Arc::new(MintPgDatabase::new(url.as_str()).await.expect("pg db b"));

    let seed = b"test-seed-cross-instance-pg-rotations";
    let instance_a = Arc::new(
        DbSignatory::new(store_a, seed, Default::default(), Default::default())
            .await
            .expect("DbSignatory::new a"),
    );
    let instance_b = Arc::new(
        DbSignatory::new(store_b, seed, Default::default(), Default::default())
            .await
            .expect("DbSignatory::new b"),
    );

    const ROTATIONS: usize = 8;
    let mut handles = Vec::with_capacity(ROTATIONS);
    for i in 0..ROTATIONS {
        let signatory = if i % 2 == 0 {
            instance_a.clone()
        } else {
            instance_b.clone()
        };
        handles.push(tokio::spawn(async move {
            signatory
                .rotate_keyset(RotateKeyArguments {
                    unit: CurrencyUnit::Sat,
                    amounts: vec![1, 2, 4, 8],
                    input_fee_ppk: 0,
                    keyset_id_type: KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("rotate_keyset")
        }));
    }

    let mut ids = HashSet::new();
    let mut versions = HashSet::new();
    for handle in handles {
        let keyset = handle.await.expect("join");
        assert!(
            ids.insert(keyset.id),
            "duplicate keyset id from concurrent cross-instance rotation"
        );
        assert!(
            versions.insert(keyset.version),
            "duplicate path index from concurrent cross-instance rotation"
        );
    }
    assert_eq!(ids.len(), ROTATIONS);
}
