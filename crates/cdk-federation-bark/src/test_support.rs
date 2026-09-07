//! Shared storage-backend conformance checks for the Bark projection.

use cdk_common::database::mint::Database;
use cdk_common::database::Error as DatabaseError;
use cdk_common::federation::{
    FederationId, FederationJournalAppendOutcome, FederationOperation, FederationOperationEnvelope,
    FederationOperationSignature, InMemoryFederationJournal, MemberConfigPrivate, MemberId,
};
use cdk_common::{Amount, SecretKey};

use crate::{
    federation_bark_application_id, federation_bark_application_operation,
    federation_bark_description_hash, federation_bark_mailbox_scope, FederationBarkActionId,
    FederationBarkDigest, FederationBarkProjectionSnapshot, FederationBarkReceiveIntent,
    FederationBarkReceiveProof, FederationBarkReceiveSecretAcknowledgement, FederationBarkUnit,
    FederationBarkVtxoLocator, FEDERATION_BARK_OPERATION_VERSION,
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
    DB: Database<DatabaseError>,
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
        FederationOperation::ExternalApplication(application) => application.application(),
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
    let first: FederationBarkProjectionSnapshot = serde_json::from_value(
        db.get_federation_application_projection_snapshot(
            federation_bark_application_id().expect("Bark application ID"),
        )
        .await
        .expect("Bark projection snapshot"),
    )
    .expect("decode Bark projection snapshot");
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
    let second: FederationBarkProjectionSnapshot = serde_json::from_value(
        db.get_federation_application_projection_snapshot(
            federation_bark_application_id().expect("Bark application ID"),
        )
        .await
        .expect("replayed Bark projection snapshot"),
    )
    .expect("decode replayed Bark projection snapshot");
    assert_eq!(second, first);
}

/// Compare focused action reads with complete snapshots at increasing history sizes.
///
/// Reports elapsed read time and returned row counts without timing assertions.
/// Both storage backends run the same absence, rollback, replay, and equality checks.
pub async fn federation_bark_action_reads<DB>(db: DB)
where
    DB: Database<DatabaseError>,
{
    let application_id = federation_bark_application_id().expect("Bark application ID");
    let selected_id = FederationBarkActionId::from_bytes([1; 32]).expect("selected action ID");
    assert!(db
        .get_federation_application_action(application_id, selected_id.to_bytes())
        .await
        .expect("absent action read")
        .is_none());
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
            FederationOperation::ExternalApplication(application) => application.application(),
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
            .get_federation_application_action(application_id, action_id.to_bytes())
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
            let snapshot: FederationBarkProjectionSnapshot = serde_json::from_value(
                db.get_federation_application_projection_snapshot(application_id)
                    .await
                    .expect("snapshot"),
            )
            .expect("typed snapshot");
            snapshot_elapsed += started.elapsed();
            rows = snapshot.actions.len()
                + snapshot.vtxos.len()
                + snapshot.derivation_allocations.len()
                + snapshot.reservations.len()
                + snapshot.transitions.len()
                + snapshot.observations.len();
            let started = std::time::Instant::now();
            let action: crate::FederationBarkActionRecord = serde_json::from_value(
                db.get_federation_application_action(application_id, selected_id.to_bytes())
                    .await
                    .expect("focused action")
                    .expect("present action"),
            )
            .expect("typed action");
            action_elapsed += started.elapsed();
            assert_eq!(snapshot.actions.len(), index as usize);
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
}
