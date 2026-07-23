//! End-to-end checks that keyset creation, activation, and boot-time
//! re-activation emit append-only journal events through the journaling
//! database wrapper.
//!
//! These drive [`DbSignatory`] directly (with a fixed seed and a file-backed
//! sqlite database) so the boot-time re-activation path in `init_keysets` is
//! exercised deterministically. That path previously wrote no journal event; it
//! must now, because journaling is a property of the storage layer.

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::event_log::{Delta, Entity, Event, Snapshot};
use cdk_common::nut02::KeySetVersion;
use cdk_common::nuts::CurrencyUnit;
use cdk_signatory::db_signatory::DbSignatory;
use cdk_signatory::signatory::{RotateKeyArguments, Signatory};

use crate::test_helpers::mint::read_journal;

const SEED: &[u8] = b"keyset-journal-test-fixed-seed-000000000000";

fn sat_units() -> HashMap<CurrencyUnit, (u64, Vec<u64>)> {
    let mut units = HashMap::new();
    units.insert(CurrencyUnit::Sat, (0u64, vec![1, 2, 4, 8]));
    units
}

fn rotate_args() -> RotateKeyArguments {
    RotateKeyArguments {
        unit: CurrencyUnit::Sat,
        amounts: vec![1, 2, 4, 8],
        input_fee_ppk: 0,
        keyset_id_type: KeySetVersion::Version00,
        final_expiry: None,
    }
}

async fn open_db(file: &str) -> Arc<cdk_sqlite::mint::MintSqliteDatabase> {
    Arc::new(
        cdk_sqlite::mint::MintSqliteDatabase::new(file)
            .await
            .expect("file-backed mint db"),
    )
}

/// Rotating a keyset twice journals a snapshot and activation for each keyset,
/// plus a deactivation of the keyset that the second rotation supersedes.
/// Re-opening the database with the same seed re-activates the highest-index
/// keyset at boot, which must append its own journal events.
#[tokio::test]
async fn keyset_rotation_and_boot_reactivation_are_journaled() {
    let file = format!(
        "{}/cdk_journal_keyset_test.sqlite",
        std::env::temp_dir().display()
    );
    let _ = std::fs::remove_file(&file);

    // First signatory: create two keysets; the second supersedes the first.
    let (first_id, second_id) = {
        let signatory = DbSignatory::new(open_db(&file).await, SEED, sat_units(), HashMap::new())
            .await
            .expect("signatory");

        let first = signatory
            .rotate_keyset(rotate_args())
            .await
            .expect("rotate 1");
        let second = signatory
            .rotate_keyset(rotate_args())
            .await
            .expect("rotate 2");
        (first.id, second.id)
    };

    let after_rotations = read_journal(&file);

    // Both keysets have a creation snapshot.
    for id in [first_id, second_id] {
        let record = id.to_string();
        assert!(
            after_rotations.iter().any(|(entity, r, e)| *entity == Entity::Keyset
                && *r == record
                && matches!(e, Event::Snapshot(s) if matches!(s.as_ref(), Snapshot::Keyset(_)))),
            "missing keyset snapshot for {record}, journal: {after_rotations:?}"
        );
    }

    // The second rotation deactivates the first keyset.
    let first_record = first_id.to_string();
    assert!(
        after_rotations
            .iter()
            .any(|(entity, r, e)| *entity == Entity::Keyset
                && *r == first_record
                && matches!(e, Event::Delta(Delta::KeysetActive(false)))),
        "superseded keyset must be journaled as deactivated, journal: {after_rotations:?}"
    );

    // The second keyset is activated.
    let second_record = second_id.to_string();
    assert!(
        after_rotations
            .iter()
            .any(|(entity, r, e)| *entity == Entity::Keyset
                && *r == second_record
                && matches!(e, Event::Delta(Delta::KeysetActive(true)))),
        "new keyset must be journaled as activated, journal: {after_rotations:?}"
    );

    // Second signatory on the same database and seed: `init_keysets` re-activates
    // the highest-index keyset at boot. This path emitted no journal event before
    // journaling moved into the storage layer; it must now.
    {
        let _signatory = DbSignatory::new(open_db(&file).await, SEED, sat_units(), HashMap::new())
            .await
            .expect("reboot signatory");
    }

    let after_reboot = read_journal(&file);
    assert!(
        after_reboot.len() > after_rotations.len(),
        "boot-time keyset re-activation must append journal events (was {}, now {})",
        after_rotations.len(),
        after_reboot.len()
    );
    assert!(
        after_reboot[after_rotations.len()..]
            .iter()
            .all(|(entity, _, _)| *entity == Entity::Keyset),
        "the new boot events must all be keyset entries, tail: {:?}",
        &after_reboot[after_rotations.len()..]
    );

    let _ = std::fs::remove_file(&file);
}

/// The decorator rejects direct `add_journal` calls on both transaction kinds:
/// journaling is driven by the entity mutations, so a direct write from outside
/// the decorator is a programming error and must fail rather than produce an
/// unmanaged journal row.
#[tokio::test]
async fn direct_add_journal_through_wrapper_is_rejected() {
    use cdk_common::database::event_log::{Delta, Event};
    use cdk_common::database::{
        Error as DbError, JournaledDatabase, MintDatabase, MintKeysDatabase,
    };

    // Mint transaction path.
    let journaled = JournaledDatabase::new(Arc::new(
        cdk_sqlite::mint::memory::empty().await.expect("mint db"),
    ));
    let mut tx = MintDatabase::begin_transaction(&journaled)
        .await
        .expect("begin mint tx");
    let err = tx
        .add_journal("record".to_string(), Event::Delta(Delta::ProofRemoved))
        .await
        .expect_err("direct add_journal must fail");
    assert!(matches!(err, DbError::JournalNotPermitted), "got {err:?}");
    tx.rollback().await.expect("rollback mint tx");

    // Keyset transaction path.
    let journaled_keys = JournaledDatabase::new(Arc::new(
        cdk_sqlite::mint::memory::empty().await.expect("keys db"),
    ));
    let mut ktx = MintKeysDatabase::begin_transaction(&journaled_keys)
        .await
        .expect("begin keys tx");
    let kerr = ktx
        .add_journal(
            "record".to_string(),
            Event::Delta(Delta::KeysetActive(true)),
        )
        .await
        .expect_err("direct add_journal must fail");
    assert!(matches!(kerr, DbError::JournalNotPermitted), "got {kerr:?}");
    ktx.rollback().await.expect("rollback keys tx");
}
