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
    let FederationOperation::Application(application) = &entry.envelope.operation else {
        panic!("expected application operation");
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
