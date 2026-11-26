//! Shared storage-backend conformance checks for the Bark projection.

use cdk_common::database::mint::Database;
use cdk_common::database::Error as DatabaseError;
use cdk_common::federation::{
    FederationId, FederationJournalAppendOutcome, FederationOperation, FederationOperationEnvelope,
    FederationOperationSignature, InMemoryFederationJournal, MemberConfigPrivate, MemberId,
};
use cdk_common::{Amount, SecretKey};

use crate::{
    federation_bark_application_operation, federation_bark_description_hash,
    federation_bark_mailbox_scope, FederationBarkActionId, FederationBarkDigest,
    FederationBarkProjectionSnapshot, FederationBarkReceiveIntent, FederationBarkReceiveProof,
    FederationBarkReceiveSecretAcknowledgement, FederationBarkUnit, FederationBarkVtxoLocator,
    FEDERATION_BARK_OPERATION_VERSION,
};

fn federation_id() -> FederationId {
    FederationId::from_bytes([42; 32])
}

fn member_id() -> MemberId {
    MemberId::new(1).expect("member ID")
}

fn private_member() -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: Some(federation_id()),
        member_id: member_id(),
        identity_secret_key: SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        keyset_secret_shares: Default::default(),
        setup_transcript_hash: None,
    }
}

fn digest(seed: u8) -> FederationBarkDigest {
    FederationBarkDigest::from_bytes([seed; 32]).expect("Bark digest")
}

fn receive_intent() -> FederationBarkReceiveIntent {
    let mailbox_id = [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ];
    FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action ID"),
        quote: "bark-projection-quote".to_string(),
        actor: member_id(),
        cashu_amount: Amount::from(100),
        gross_amount: Amount::from(100),
        receive_fee: Amount::ZERO,
        minimum_net_amount: Amount::from(100),
        unit: FederationBarkUnit::Sat,
        description_hash: federation_bark_description_hash(None).expect("description hash"),
        description: None,
        payment_hash: digest(2),
        preimage_commitment: digest(3),
        secret_acknowledgements: vec![FederationBarkReceiveSecretAcknowledgement {
            member: member_id(),
            record_hash: digest(5),
            signature: FederationOperationSignature::from_slice(&[6; 64]).expect("signature bytes"),
        }],
        destination: FederationBarkVtxoLocator::vtxo_custody(7),
        mailbox_scope: federation_bark_mailbox_scope(&mailbox_id).expect("mailbox scope"),
        mailbox_id,
        ark_server: digest(4),
        network: bitcoin::Network::Regtest,
        minimum_cltv_delta: 18,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at: 1_900_000_000,
    }
}

/// Assert that an application-wrapped Bark operation persists and replays idempotently.
pub async fn federation_bark_projection_persistence<DB>(db: DB)
where
    DB: Database<DatabaseError> + crate::FederationBarkStore,
{
    let operation = crate::FederationBarkOperation::ReceiveIntent(receive_intent());
    let envelope = FederationOperationEnvelope::new(
        federation_id(),
        federation_bark_application_operation(operation).expect("Bark application operation"),
    )
    .authorize(&private_member())
    .expect("authorize Bark operation");
    let mut journal = InMemoryFederationJournal::new();
    let entry = match journal.append(envelope).expect("append Bark operation") {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => entry,
    };
    let application = match &entry.envelope.operation {
        FederationOperation::Application(application) => application,
        _ => panic!("expected application operation"),
    };

    let mut transaction = Database::begin_transaction(&db)
        .await
        .expect("begin Bark projection transaction");
    transaction
        .apply_federation_application_operation(&entry, application)
        .await
        .expect("apply Bark projection operation");
    transaction
        .commit()
        .await
        .expect("commit Bark projection operation");
    let first: FederationBarkProjectionSnapshot = db
        .bark_projection_snapshot()
        .await
        .expect("Bark projection snapshot");
    assert_eq!(first.actions.len(), 1);
    assert!(first.transitions.is_empty());

    let mut replay = Database::begin_transaction(&db)
        .await
        .expect("begin Bark replay transaction");
    replay
        .apply_federation_application_operation(&entry, application)
        .await
        .expect("replay Bark projection operation");
    replay.commit().await.expect("commit Bark replay");
    let second: FederationBarkProjectionSnapshot = db
        .bark_projection_snapshot()
        .await
        .expect("replayed Bark projection snapshot");
    assert_eq!(second, first);
}

fn observation_entry(
    journal: &mut InMemoryFederationJournal,
    observation_epoch: u64,
) -> cdk_common::federation::FederationJournalEntry {
    let observation =
        crate::FederationBarkOperation::MemberObservation(crate::FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action ID"),
            quote: Some("bark-projection-quote".to_string()),
            observer: member_id(),
            observation_epoch,
            source_scope: digest(8),
            payment_hash: None,
            status: crate::FederationBarkObservationStatus::Pending,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            result_hash: digest(9),
        });
    let envelope = FederationOperationEnvelope::new(
        federation_id(),
        federation_bark_application_operation(observation).expect("Bark application operation"),
    )
    .authorize(&private_member())
    .expect("authorize Bark observation");
    match journal.append(envelope).expect("append Bark observation") {
        FederationJournalAppendOutcome::Appended(entry) => entry,
        FederationJournalAppendOutcome::Duplicate(_) => panic!("unique observation"),
    }
}

async fn apply_entry<DB>(db: &DB, entry: &cdk_common::federation::FederationJournalEntry)
where
    DB: Database<DatabaseError>,
{
    let FederationOperation::Application(application) = &entry.envelope.operation else {
        panic!("application operation");
    };
    let mut transaction = Database::begin_transaction(db)
        .await
        .expect("begin Bark apply");
    transaction
        .apply_federation_application_operation(entry, application)
        .await
        .expect("apply Bark operation");
    transaction.commit().await.expect("commit Bark apply");
}

/// Assert that a retired action guards against replays and late operations
/// whether or not its transitions and observations were deleted, and that
/// only those rows are deleted below the retention floor.
///
/// `source` records a receive and one observation of it. `installed` holds
/// the same rows with the action finished and its quote settled, as a state
/// transfer installs them.
pub async fn federation_bark_retired_action_guard<DB>(source: DB, installed: DB)
where
    DB: Database<DatabaseError> + crate::FederationBarkStore,
{
    let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
    let mut journal = InMemoryFederationJournal::new();
    let intent = FederationOperationEnvelope::new(
        federation_id(),
        federation_bark_application_operation(crate::FederationBarkOperation::ReceiveIntent(
            receive_intent(),
        ))
        .expect("Bark application operation"),
    )
    .authorize(&private_member())
    .expect("authorize Bark operation");
    let intent_entry = match journal.append(intent).expect("append receive") {
        FederationJournalAppendOutcome::Appended(entry) => entry,
        FederationJournalAppendOutcome::Duplicate(_) => panic!("unique receive"),
    };
    let observed = observation_entry(&mut journal, 1);
    apply_entry(&source, &intent_entry).await;
    apply_entry(&source, &observed).await;

    // Settling the quote of an unfinished action only records it.
    let mut transaction = Database::begin_transaction(&source)
        .await
        .expect("begin settle");
    transaction
        .settle_federation_application_quote("bark-projection-quote", 1)
        .await
        .expect("settle quote");
    transaction.commit().await.expect("commit settle");
    let action = source
        .bark_action(action_id)
        .await
        .expect("action read")
        .expect("action");
    assert_eq!(action.quote_settled_operation_index, Some(1));
    assert_eq!(action.retired_operation_index(), None);
    let snapshot = source.bark_projection_snapshot().await.expect("snapshot");
    assert_eq!(snapshot.observations.len(), 1);

    // The same rows with the action finished: retired at index 1.
    let mut finished = snapshot.clone();
    finished.actions[0].phase = crate::FederationBarkSemanticPhase::ReceiveClaimed;
    assert_eq!(finished.actions[0].retired_operation_index(), Some(1));
    let mut transaction = Database::begin_transaction(&installed)
        .await
        .expect("begin install");
    transaction
        .install_federation_application_rows(
            &finished.materialized_rows().expect("Bark rows to install"),
        )
        .await
        .expect("install Bark rows");
    transaction.commit().await.expect("commit install");
    assert_eq!(
        installed
            .bark_projection_snapshot()
            .await
            .expect("installed snapshot"),
        finished
    );

    // A late observation of the retired action changes nothing.
    let late = observation_entry(&mut journal, 2);
    apply_entry(&installed, &late).await;
    assert_eq!(
        installed
            .bark_projection_snapshot()
            .await
            .expect("snapshot after a late observation"),
        finished
    );

    // Its rows are deleted only below the floor, and the action stays.
    for (floor, deleted) in [(1, 0), (2, 1), (2, 0)] {
        let mut transaction = Database::begin_transaction(&installed)
            .await
            .expect("begin prune");
        assert_eq!(
            transaction
                .prune_federation_terminal_rows(floor, 100)
                .await
                .expect("prune retired rows"),
            deleted
        );
        transaction.commit().await.expect("commit prune");
    }
    let pruned = installed
        .bark_projection_snapshot()
        .await
        .expect("pruned snapshot");
    assert!(pruned.observations.is_empty());
    assert_eq!(pruned.actions, finished.actions);

    // A replay of the deleted observation does not bring it back.
    apply_entry(&installed, &observed).await;
    assert_eq!(
        installed
            .bark_projection_snapshot()
            .await
            .expect("snapshot after a replay"),
        pruned
    );
}

/// Compare focused action reads with complete snapshots at increasing history sizes.
///
/// Reports elapsed read time and returned row counts without timing assertions.
/// Both storage backends run the same absence, rollback, replay, and equality checks.
pub async fn federation_bark_action_reads<DB>(db: DB)
where
    DB: Database<DatabaseError> + crate::FederationBarkStore,
{
    let selected_id = FederationBarkActionId::from_bytes([1; 32]).expect("selected action ID");
    assert!(db
        .bark_action(selected_id)
        .await
        .expect("absent action read")
        .is_none());
    assert_eq!(
        db.bark_projection_frontier().await.expect("empty frontier"),
        None
    );
    let mut journal = InMemoryFederationJournal::new();
    for index in 1_u64..=256 {
        let mut intent = receive_intent();
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&index.to_be_bytes());
        intent.action_id = match index {
            1 => selected_id,
            _ => FederationBarkActionId::from_bytes(bytes).expect("action ID"),
        };
        intent.quote = format!("bark-action-read-{index}");
        intent.payment_hash = FederationBarkDigest::from_bytes(bytes).expect("payment hash");
        intent.destination = FederationBarkVtxoLocator::vtxo_custody(
            u32::try_from(index).expect("bounded history index"),
        );
        let envelope = FederationOperationEnvelope::new(
            federation_id(),
            federation_bark_application_operation(crate::FederationBarkOperation::ReceiveIntent(
                intent,
            ))
            .expect("Bark application operation"),
        )
        .authorize(&private_member())
        .expect("authorize Bark operation");
        let entry = match journal.append(envelope).expect("append action") {
            FederationJournalAppendOutcome::Appended(entry) => entry,
            FederationJournalAppendOutcome::Duplicate(_) => panic!("unique action"),
        };
        let application = match &entry.envelope.operation {
            FederationOperation::Application(application) => application,
            _ => panic!("application operation"),
        };
        let mut transaction = Database::begin_transaction(&db)
            .await
            .expect("begin action");
        transaction
            .apply_federation_application_operation(&entry, application)
            .await
            .expect("apply action");
        transaction.rollback().await.expect("rollback action");
        let action_id = crate::validate_federation_bark_operation(&entry.envelope.operation)
            .expect("Bark operation")
            .action_id();
        assert!(db
            .bark_action(action_id)
            .await
            .expect("rolled back action read")
            .is_none());
        let mut transaction = Database::begin_transaction(&db)
            .await
            .expect("begin action");
        transaction
            .apply_federation_application_operation(&entry, application)
            .await
            .expect("apply action");
        transaction
            .apply_federation_application_operation(&entry, application)
            .await
            .expect("idempotent replay");
        transaction.commit().await.expect("commit action");
        if ![1, 64, 256].contains(&index) {
            continue;
        }
        let mut snapshot_elapsed = std::time::Duration::ZERO;
        let mut action_elapsed = std::time::Duration::ZERO;
        let mut rows = 0;
        let mut last_snapshot = None;
        for _ in 0..20 {
            let started = std::time::Instant::now();
            let snapshot: FederationBarkProjectionSnapshot =
                db.bark_projection_snapshot().await.expect("snapshot");
            snapshot_elapsed += started.elapsed();
            rows = snapshot.actions.len()
                + snapshot.vtxos.len()
                + snapshot.derivation_allocations.len()
                + snapshot.reservations.len()
                + snapshot.transitions.len()
                + snapshot.observations.len();
            let started = std::time::Instant::now();
            let action: crate::FederationBarkActionRecord = db
                .bark_action(selected_id)
                .await
                .expect("focused action")
                .expect("present action");
            action_elapsed += started.elapsed();
            assert_eq!(snapshot.actions.len(), index as usize);
            assert_eq!(
                db.bark_projection_frontier().await.expect("frontier"),
                snapshot.frontier()
            );
            assert_eq!(
                Some(&action),
                snapshot.actions.iter().find(|a| a.action_id == selected_id)
            );
            last_snapshot = Some(snapshot);
        }
        let digest = cdk_common::federation::FederationMaterializedStateSnapshot::from_rows(
            last_snapshot
                .expect("measured snapshot")
                .materialized_rows()
                .expect("canonical rows"),
        )
        .digest()
        .expect("materialized digest");
        eprintln!("Bark read digest: history={index}, digest={digest}");
        eprintln!("Bark read comparison: history={index}, repetitions=20, snapshot_rows={rows}, focused_rows=1, snapshot_us={}, focused_us={}", snapshot_elapsed.as_micros(), action_elapsed.as_micros());
    }
    assert_keyed_reads_match_snapshot(&db, selected_id).await;
}

/// Check every keyed store read against the same filter over the whole snapshot.
async fn assert_keyed_reads_match_snapshot<DB>(db: &DB, action_id: FederationBarkActionId)
where
    DB: crate::FederationBarkStore,
{
    let snapshot = db.bark_projection_snapshot().await.expect("snapshot");
    let mut transitions = snapshot
        .transitions
        .iter()
        .filter(|record| record.action_id == action_id)
        .cloned()
        .collect::<Vec<_>>();
    transitions.sort_by_key(|record| record.operation_index);
    assert_eq!(
        db.bark_action_transitions(action_id)
            .await
            .expect("action transitions"),
        transitions
    );
    let mut observations = snapshot
        .observations
        .iter()
        .filter(|record| {
            matches!(
                &record.operation,
                crate::FederationBarkOperation::MemberObservation(observation)
                    if observation.action_id == action_id
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    observations.sort_by_key(|record| record.operation_index);
    assert_eq!(
        db.bark_action_observations(action_id)
            .await
            .expect("action observations"),
        observations
    );
    for state in [
        crate::FederationBarkVtxoState::Observed,
        crate::FederationBarkVtxoState::Spendable,
        crate::FederationBarkVtxoState::Reserved,
        crate::FederationBarkVtxoState::PendingSpend,
        crate::FederationBarkVtxoState::Spent,
        crate::FederationBarkVtxoState::Exiting,
        crate::FederationBarkVtxoState::Exited,
    ] {
        let expected = snapshot
            .vtxos
            .iter()
            .filter(|record| record.state == state)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            db.bark_vtxos_in_state(state).await.expect("VTXOs in state"),
            expected
        );
    }
    let reserved = snapshot
        .vtxos
        .iter()
        .filter(|record| record.reservation_action_id == Some(action_id))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        db.bark_vtxos_reserved_by(action_id)
            .await
            .expect("reserved VTXOs"),
        reserved
    );
    let ids = snapshot
        .vtxos
        .iter()
        .map(|record| record.vtxo_id)
        .collect::<Vec<_>>();
    assert_eq!(
        db.bark_vtxos(&ids).await.expect("VTXOs by id"),
        snapshot.vtxos
    );
    assert!(db.bark_vtxos(&[]).await.expect("no VTXOs").is_empty());
    assert_eq!(
        db.bark_max_locator_index().await.expect("max locator"),
        snapshot.max_locator_index()
    );
}
