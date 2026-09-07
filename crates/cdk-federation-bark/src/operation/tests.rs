use std::time::{Duration, UNIX_EPOCH};

use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

use super::*;
use cdk_common::federation::{
    FederationAddressKeychain, FederationConfigDigest, FederationConsensusItem,
    FederationJournalAppendOutcome, FederationOperationEnvelope, InMemoryFederationJournal,
};

use crate::{
    FederationBarkEvidenceAttestation, FederationBarkPolicyConfig,
    FederationBarkReceiveAntiDosPolicy,
};

fn digest(seed: u8) -> FederationBarkDigest {
    FederationBarkDigest::from_bytes([seed; 32]).expect("non-zero digest")
}

fn action(seed: u8) -> FederationBarkActionId {
    FederationBarkActionId::from_bytes([seed; 32]).expect("non-zero action")
}

fn vtxo(seed: u8) -> FederationBarkVtxoId {
    FederationBarkVtxoId::from_bytes([seed; 36]).expect("non-zero VTXO")
}

fn external(seed: u8) -> FederationBarkExternalId {
    FederationBarkExternalId::from_bytes([seed; 32]).expect("non-zero external ID")
}

fn member(value: u16) -> MemberId {
    MemberId::new(value).expect("member")
}

fn certified_receive_claim_plan() -> FederationBarkActionPlan {
    let action_id = action(80);
    let predecessor_operation_id = operation_id(81);
    let payment_hash = digest(82);
    let vtxo_artifact =
        bark_artifact(FederationBarkArtifactKind::Vtxo, vec![83; 32]).expect("VTXO artifact");
    let request_artifact = bark_artifact(
        FederationBarkArtifactKind::ArkRequest,
        b"certified-receive-claim".to_vec(),
    )
    .expect("claim request artifact");
    let input = FederationBarkVtxoInput {
        vtxo_id: vtxo(84),
        locator: FederationBarkVtxoLocator::vtxo_custody(7),
        amount: Amount::from(100),
    };
    let transition = FederationBarkVtxoTransition {
        vtxo_id: input.vtxo_id,
        locator: input.locator,
        amount: input.amount,
        expiry_height: 850_000,
        previous_state: None,
        new_state: FederationBarkVtxoState::Observed,
        artifact_hash: vtxo_artifact.hash,
    };
    let request_hash = digest(88);
    let mut artifacts = vec![request_artifact.clone(), vtxo_artifact];
    artifacts.sort();
    let evidence_digest = federation_bark_receive_htlc_package_hash(
        850_000,
        request_hash,
        core::slice::from_ref(&transition),
        &artifacts,
    )
    .expect("receive package");
    let certificate = FederationBarkEvidenceCertificate {
        binding: crate::FederationBarkEvidenceBinding {
            version: crate::FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION,
            federation_id: federation_id(),
            config_digest: FederationConfigDigest::from_bytes([85; 32]),
            application_key_epoch: 1,
            action_id,
            quote: "certified-receive".to_string(),
            phase: FederationBarkEvidencePhase::ReceiveHtlcsReady,
            evidence_operation_id: predecessor_operation_id,
            payment_hash,
            source_scope: digest(86),
            status: FederationBarkObservationStatus::Accepted,
            amount: Amount::from(100),
            fee: Amount::ZERO,
            evidence_digest,
            attempt: 1,
        },
        attestations: vec![FederationBarkEvidenceAttestation {
            signer: member(1),
            signature: FederationOperationSignature::from_slice(&[1; 64]).expect("signature"),
        }],
    };
    FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id,
        quote: Some("certified-receive".to_string()),
        kind: FederationBarkActionKind::IncomingClaim,
        actor: member(1),
        predecessor_operation_id,
        payment_hash: Some(payment_hash),
        inputs: vec![input],
        outputs: vec![FederationBarkVtxoOutput {
            locator: FederationBarkVtxoLocator::vtxo_custody(8),
            amount: Amount::from(100),
            artifact_hash: request_artifact.hash,
        }],
        revocation_locator: None,
        external_amount: Amount::ZERO,
        fee_amount: Amount::ZERO,
        fee_bound: Amount::ZERO,
        artifacts,
        certified_receive_htlcs: Some(Box::new(FederationBarkCertifiedReceiveHtlcs {
            version: FEDERATION_BARK_OPERATION_VERSION,
            minimum_expiry: 850_000,
            request_hash,
            vtxo_transitions: vec![transition],
            certificate,
        })),
        external_id: external(89),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        expires_at: 850_000,
    }
}

#[test]
fn certified_receive_claim_is_one_authority_free_operation() {
    let plan = certified_receive_claim_plan();
    let operation = FederationBarkOperation::ActionPlan(plan.clone());
    operation.validate().expect("combined receive claim");
    assert_eq!(operation.claimed_author(), None);
    assert!(operation
        .conflict_keys()
        .contains(&FederationBarkConflictKey::ActionPhase {
            action_id: plan.action_id,
            phase: FederationBarkSemanticPhase::ReceiveHtlcsReady,
            attempt: Some(1),
        }));
    assert!(operation
        .conflict_keys()
        .contains(&FederationBarkConflictKey::EvidenceCertificate {
            action_id: plan.action_id,
            phase: FederationBarkEvidencePhase::ReceiveHtlcsReady,
        }));
}

#[test]
fn certified_receive_claim_mutations_fail_closed() {
    let base = certified_receive_claim_plan();
    let mut mutations = Vec::new();
    let mut changed = base.clone();
    changed
        .certified_receive_htlcs
        .as_mut()
        .expect("certified")
        .certificate
        .binding
        .amount = Amount::from(99);
    mutations.push(changed);
    let mut changed = base.clone();
    changed
        .certified_receive_htlcs
        .as_mut()
        .expect("certified")
        .minimum_expiry -= 1;
    mutations.push(changed);
    let mut changed = base.clone();
    changed.attempt += 1;
    mutations.push(changed);
    let mut changed = base.clone();
    changed
        .certified_receive_htlcs
        .as_mut()
        .expect("certified")
        .vtxo_transitions[0]
        .artifact_hash = digest(90);
    mutations.push(changed);
    let mut changed = base.clone();
    let replacement = bark_artifact(FederationBarkArtifactKind::Vtxo, vec![91; 32])
        .expect("replacement VTXO artifact");
    changed
        .artifacts
        .retain(|artifact| artifact.kind != FederationBarkArtifactKind::Vtxo);
    changed.artifacts.push(replacement.clone());
    changed.artifacts.sort();
    changed
        .certified_receive_htlcs
        .as_mut()
        .expect("certified")
        .vtxo_transitions[0]
        .artifact_hash = replacement.hash;
    mutations.push(changed);
    let mut changed = base;
    changed
        .certified_receive_htlcs
        .as_mut()
        .expect("certified")
        .certificate
        .binding
        .status = FederationBarkObservationStatus::Failed;
    mutations.push(changed);

    for mutation in mutations {
        assert!(FederationBarkOperation::ActionPlan(mutation)
            .validate()
            .is_err());
    }
}

#[test]
fn signing_retry_transition_is_canonical_and_preserves_action_identity() {
    let plan = FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: action(41),
        quote: Some("retry-quote".to_string()),
        kind: FederationBarkActionKind::IncomingClaim,
        actor: member(1),
        predecessor_operation_id: FederationOperationId::from_bytes([42; 32]),
        payment_hash: Some(digest(43)),
        inputs: Vec::new(),
        outputs: Vec::new(),
        revocation_locator: None,
        external_amount: Amount::ZERO,
        fee_amount: Amount::ZERO,
        fee_bound: Amount::ZERO,
        certified_receive_htlcs: None,
        artifacts: Vec::new(),
        external_id: external(44),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        expires_at: 1,
    };
    let plan_operation_id = FederationOperationId::from_bytes([45; 32]);
    let predecessor = FederationOperationId::from_bytes([46; 32]);
    let first = FederationBarkActionTransition::signing_attempt_advanced(
        plan_operation_id,
        predecessor,
        &plan,
        plan.actor,
        2,
        vec![member(2), member(3)],
    )
    .expect("retry transition");
    let replay = FederationBarkActionTransition::signing_attempt_advanced(
        plan_operation_id,
        predecessor,
        &plan,
        plan.actor,
        2,
        vec![member(2), member(3)],
    )
    .expect("exact retry replay");
    let fallback = FederationBarkActionTransition::signing_attempt_advanced(
        plan_operation_id,
        predecessor,
        &plan,
        member(2),
        2,
        vec![member(2), member(3)],
    )
    .expect("actor replacement transition");

    assert_eq!(first, replay);
    assert_eq!(fallback.actor, member(2));
    assert_ne!(fallback.result_hash, first.result_hash);
    assert_eq!(
        FederationBarkOperation::ActionTransition(fallback).claimed_author(),
        Some(member(2)),
    );
    assert_eq!(first.external_id, plan.external_id);
    assert_eq!(
        first.request_hash,
        FederationBarkOperation::ActionPlan(plan)
            .commitment()
            .expect("plan commitment")
    );
    assert!(first.artifacts.is_empty() && first.vtxo_transitions.is_empty());
}

fn operation_id(seed: u8) -> FederationOperationId {
    FederationOperationId::from_bytes([seed; 32])
}

fn ark_server_key() -> [u8; 33] {
    [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ]
}

fn federation_id() -> FederationId {
    FederationId::from_bytes([42; 32])
}

fn acknowledgement(member_id: u16) -> FederationBarkReceiveSecretAcknowledgement {
    FederationBarkReceiveSecretAcknowledgement {
        member: member(member_id),
        record_hash: digest(12),
        signature: FederationOperationSignature::from_slice(&[member_id as u8; 64])
            .expect("signature"),
    }
}

fn receive_intent() -> FederationBarkOperation {
    FederationBarkOperation::ReceiveIntent(FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: action(1),
        quote: "mint-quote-1".to_string(),
        actor: member(1),
        cashu_amount: Amount::from(100),
        gross_amount: Amount::from(100),
        receive_fee: Amount::ZERO,
        minimum_net_amount: Amount::from(100),
        unit: FederationBarkUnit::Sat,
        description_hash: federation_bark_description_hash(None).expect("description hash"),
        description: None,
        payment_hash: digest(3),
        preimage_commitment: digest(4),
        secret_acknowledgements: vec![acknowledgement(1), acknowledgement(2)],
        destination: FederationBarkVtxoLocator::vtxo_custody(7),
        mailbox_scope: federation_bark_mailbox_scope(&ark_server_key()).expect("mailbox scope"),
        mailbox_id: ark_server_key(),
        ark_server: digest(6),
        network: Network::Regtest,
        minimum_cltv_delta: 18,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at: 1_900_000_000,
    })
}

fn receive_prepared() -> FederationBarkOperation {
    FederationBarkOperation::ReceivePrepared(FederationBarkReceivePrepared {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: action(1),
        quote: "mint-quote-1".to_string(),
        actor: member(2),
        intent_operation_id: operation_id(1),
        invoice: "lnbcrt1canonicalinvoice".to_string(),
        payment_hash: digest(3),
        gross_amount: Amount::from(100),
        expected_net_amount: Amount::from(100),
        network: Network::Regtest,
        expires_at: 1_900_000_000,
        final_cltv_delta: 18,
        receive_fee: Amount::ZERO,
        request_hash: digest(15),
        result_hash: digest(16),
        external_id: external(17),
    })
}

fn signed_receive_invoice(
    currency: Currency,
    payment_hash: [u8; 32],
    amount_msat: Option<u64>,
    description: &str,
    timestamp: u64,
    expiry_seconds: u64,
    final_cltv_delta: u64,
) -> Bolt11Invoice {
    let mut builder = InvoiceBuilder::new(currency)
        .description(description.to_string())
        .payment_hash(Sha256Hash::from_byte_array(payment_hash))
        .payment_secret(PaymentSecret([43; 32]))
        .timestamp(UNIX_EPOCH + Duration::from_secs(timestamp))
        .expiry_time(Duration::from_secs(expiry_seconds))
        .min_final_cltv_expiry_delta(final_cltv_delta);
    if let Some(amount_msat) = amount_msat {
        builder = builder.amount_milli_satoshis(amount_msat);
    }
    let signing_key = SecretKey::from_slice(&[42; 32]).expect("invoice signing key");
    builder
        .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &signing_key))
        .expect("signed receive invoice")
}

fn exact_receive_invoice() -> Bolt11Invoice {
    signed_receive_invoice(
        Currency::Regtest,
        [3; 32],
        Some(100_000),
        "",
        1_800_000_000,
        3_600,
        18,
    )
}

fn send_plan() -> FederationBarkOperation {
    let action_id = action(20);
    let predecessor_operation_id = operation_id(19);
    let mut plan = FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id,
        quote: Some("melt-quote-1".to_string()),
        kind: FederationBarkActionKind::LightningSend,
        actor: member(1),
        predecessor_operation_id,
        payment_hash: Some(digest(21)),
        inputs: vec![FederationBarkVtxoInput {
            vtxo_id: vtxo(22),
            locator: FederationBarkVtxoLocator::vtxo_custody(7),
            amount: Amount::from(110),
        }],
        outputs: vec![FederationBarkVtxoOutput {
            locator: FederationBarkVtxoLocator::vtxo_custody(8),
            amount: Amount::from(5),
            artifact_hash: digest(23),
        }],
        revocation_locator: Some(FederationBarkVtxoLocator::vtxo_custody(9)),
        external_amount: Amount::from(100),
        fee_amount: Amount::from(5),
        fee_bound: Amount::from(8),
        certified_receive_htlcs: None,
        artifacts: Vec::new(),
        external_id: FederationBarkExternalId::derive_lightning_send(
            action_id,
            predecessor_operation_id,
            1,
        )
        .expect("send external id"),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        expires_at: 850_000,
    };
    let invoice = signed_receive_invoice(
        Currency::Regtest,
        [21; 32],
        Some(100_000),
        "send",
        1_800_000_000,
        3_600,
        18,
    );
    plan.bind_lightning_send_artifacts(&invoice)
        .expect("bind send artifacts");
    FederationBarkOperation::ActionPlan(plan)
}

fn maintenance_input_artifact() -> FederationBarkArtifact {
    bark_artifact(FederationBarkArtifactKind::Vtxo, vec![81; 96])
        .expect("maintenance VTXO artifact")
}

fn refresh_plan() -> FederationBarkActionPlan {
    let action_id = action(60);
    let predecessor_operation_id = operation_id(59);
    let input_artifact = maintenance_input_artifact();
    let mut plan = FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id,
        quote: None,
        kind: FederationBarkActionKind::Refresh,
        actor: member(1),
        predecessor_operation_id,
        payment_hash: None,
        inputs: vec![FederationBarkVtxoInput {
            vtxo_id: vtxo(61),
            locator: FederationBarkVtxoLocator::vtxo_custody(11),
            amount: Amount::from(100),
        }],
        outputs: vec![FederationBarkVtxoOutput {
            locator: FederationBarkVtxoLocator::vtxo_custody(12),
            amount: Amount::from(98),
            artifact_hash: digest(62),
        }],
        revocation_locator: None,
        external_amount: Amount::ZERO,
        fee_amount: Amount::from(2),
        fee_bound: Amount::from(3),
        certified_receive_htlcs: None,
        artifacts: Vec::new(),
        external_id: FederationBarkExternalId::derive_refresh(
            action_id,
            predecessor_operation_id,
            1,
        )
        .expect("refresh external id"),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        expires_at: 880,
    };
    plan.bind_refresh_artifacts(800, 900, [input_artifact])
        .expect("bind refresh request");
    plan
}

fn emergency_exit_plan() -> FederationBarkActionPlan {
    let action_id = action(60);
    let predecessor_operation_id = operation_id(63);
    let input_artifact = maintenance_input_artifact();
    let mut destination_script = vec![0x51, 0x20];
    destination_script.extend_from_slice(&[64; 32]);
    let mut plan = FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id,
        quote: None,
        kind: FederationBarkActionKind::EmergencyExit,
        actor: member(1),
        predecessor_operation_id,
        payment_hash: None,
        inputs: vec![FederationBarkVtxoInput {
            vtxo_id: vtxo(61),
            locator: FederationBarkVtxoLocator::vtxo_custody(11),
            amount: Amount::from(100),
        }],
        outputs: Vec::new(),
        revocation_locator: None,
        external_amount: Amount::from(95),
        fee_amount: Amount::from(5),
        fee_bound: Amount::from(5),
        certified_receive_htlcs: None,
        artifacts: Vec::new(),
        external_id: FederationBarkExternalId::derive_emergency_exit(
            action_id,
            predecessor_operation_id,
            1,
        )
        .expect("exit external id"),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        expires_at: 900,
    };
    plan.bind_emergency_exit_artifacts(
        FederationAllocatedAddressRef {
            application_id: FederationFrostApplicationId::from_bytes([65; 32])
                .expect("application id"),
            application_key_epoch: 1,
            quote: federation_bark_exit_allocation_quote(action_id),
            keychain: FederationAddressKeychain::Internal,
            derivation_index: 7,
        },
        destination_script,
        Network::Regtest,
        FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
        850,
        2,
        [input_artifact],
        [vec![66; 128]],
    )
    .expect("bind exit request");
    plan
}

fn envelope(operation: FederationBarkOperation) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id(),
        federation_bark_application_operation(operation).expect("Bark application"),
    )
}

#[test]
fn nested_operation_is_canonical_bounded_and_roundtrips() {
    let operation = receive_intent();
    operation.validate().expect("valid receive intent");
    let canonical = operation.canonical_bytes().expect("canonical operation");
    assert!(canonical.len() < FEDERATION_MAX_BARK_OPERATION_BYTES);
    let decoded: FederationBarkOperation =
        serde_json::from_slice(&canonical).expect("decode canonical operation");
    assert_eq!(decoded, operation);
    assert_eq!(
        decoded.commitment().expect("commitment"),
        operation.commitment().expect("commitment")
    );
}

#[test]
fn maintenance_plans_bind_exact_refresh_and_exit_requests() {
    let refresh = refresh_plan();
    let refresh_operation = FederationBarkOperation::ActionPlan(refresh.clone());
    refresh_operation
        .validate()
        .expect("valid exact refresh plan");
    assert!(refresh_operation
        .conflict_keys()
        .iter()
        .any(|key| matches!(key, FederationBarkConflictKey::VtxoReservation { .. })));
    let request = refresh.refresh_request().expect("refresh request");
    assert_eq!(request.trigger_height, 800);
    assert_eq!(request.emergency_height, 880);
    assert_eq!(request.minimum_expiry, 900);

    let mut changed_refresh = refresh;
    changed_refresh.expires_at = 879;
    assert!(FederationBarkOperation::ActionPlan(changed_refresh)
        .validate()
        .is_err());

    let exit = emergency_exit_plan();
    let exit_operation = FederationBarkOperation::ActionPlan(exit.clone());
    exit_operation.validate().expect("valid exact exit plan");
    assert!(exit_operation
        .conflict_keys()
        .iter()
        .all(|key| !matches!(key, FederationBarkConflictKey::VtxoReservation { .. })));
    let mut legacy_exit = exit.clone();
    legacy_exit.version = 1;
    let legacy_exit = FederationBarkOperation::ActionPlan(legacy_exit);
    assert!(legacy_exit.validate().is_err());
    let request = exit.emergency_exit_request().expect("exit request");
    assert_eq!(
        request.destination.quote,
        federation_bark_exit_allocation_quote(exit.action_id)
    );
    assert_eq!(
        request.fee_rate_sat_per_kwu,
        FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU
    );

    let mut changed_exit = exit;
    changed_exit.fee_bound = Amount::from(6);
    assert!(FederationBarkOperation::ActionPlan(changed_exit)
        .validate()
        .is_err());
}

#[test]
fn oversized_accepted_refresh_is_replayable_but_requires_recovery() {
    let mut plan = refresh_plan();
    assert!(!federation_bark_refresh_requires_recovery(&plan));
    let template_input = plan.inputs[0];
    let template_artifact = maintenance_input_artifact();
    for seed in 1..FEDERATION_MAX_BARK_REFRESH_INPUTS {
        let mut input = template_input;
        let seed = u8::try_from(seed).expect("small test seed");
        input.vtxo_id = vtxo(seed);
        input.locator = FederationBarkVtxoLocator::vtxo_custody(u32::from(seed) + 20);
        plan.inputs.push(input);
        plan.artifacts.push(template_artifact.clone());
    }
    plan.inputs.push(FederationBarkVtxoInput {
        vtxo_id: vtxo(90),
        locator: FederationBarkVtxoLocator::vtxo_custody(90),
        amount: Amount::from(100),
    });
    plan.artifacts.push(template_artifact);

    assert!(federation_bark_refresh_requires_recovery(&plan));
}

#[test]
fn prepared_receive_requires_the_exact_signed_invoice_and_intent_commitments() {
    let FederationBarkOperation::ReceiveIntent(intent) = receive_intent() else {
        panic!("receive intent")
    };
    let intent_operation_id = operation_id(1);
    let prepared = FederationBarkReceivePrepared::from_invoice(
        member(2),
        intent_operation_id,
        &intent,
        exact_receive_invoice(),
    )
    .expect("exact prepared receive");
    prepared
        .validate_against_intent(&intent)
        .expect("exact prepared receive validation");
    assert_eq!(
        prepared.request_hash,
        federation_bark_receive_request_hash(intent_operation_id, &intent)
            .expect("request commitment")
    );

    for invoice in [
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            Some(99_000),
            "",
            1_800_000_000,
            3_600,
            18,
        ),
        signed_receive_invoice(
            Currency::Regtest,
            [44; 32],
            Some(100_000),
            "",
            1_800_000_000,
            3_600,
            18,
        ),
        signed_receive_invoice(
            Currency::Bitcoin,
            [3; 32],
            Some(100_000),
            "",
            1_800_000_000,
            3_600,
            18,
        ),
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            Some(100_000),
            "changed",
            1_800_000_000,
            3_600,
            18,
        ),
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            Some(100_000),
            "",
            1_800_000_000,
            3_600,
            17,
        ),
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            Some(100_000),
            "",
            1_899_999_000,
            3_600,
            18,
        ),
        signed_receive_invoice(
            Currency::Regtest,
            [3; 32],
            None,
            "",
            1_800_000_000,
            3_600,
            18,
        ),
    ] {
        assert!(FederationBarkReceivePrepared::from_invoice(
            member(2),
            intent_operation_id,
            &intent,
            invoice,
        )
        .is_err());
    }

    let mut changed = prepared.clone();
    changed.actor = member(3);
    assert!(changed.validate_against_intent(&intent).is_err());
    let mut changed = prepared.clone();
    changed.request_hash = digest(70);
    assert!(changed.validate_against_intent(&intent).is_err());
    let mut changed = prepared.clone();
    changed.result_hash = digest(71);
    assert!(changed.validate_against_intent(&intent).is_err());
    let mut changed = prepared;
    changed.external_id = external(72);
    assert!(changed.validate_against_intent(&intent).is_err());
}

#[test]
fn journal_replay_is_idempotent_and_rebuildable() {
    let envelope = envelope(receive_intent());
    let consensus_item = FederationConsensusItem::operation(envelope.clone());
    let consensus_wire = serde_json::to_vec(&consensus_item).expect("consensus transport bytes");
    let decoded_item: FederationConsensusItem =
        serde_json::from_slice(&consensus_wire).expect("consensus transport decode");
    assert_eq!(
        decoded_item.operation_id().expect("decoded operation ID"),
        consensus_item.operation_id().expect("operation ID")
    );
    let mut journal = InMemoryFederationJournal::new();
    assert!(matches!(
        journal.append(envelope.clone()).expect("first append"),
        FederationJournalAppendOutcome::Appended(_)
    ));
    assert!(matches!(
        journal.append(envelope).expect("duplicate append"),
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let encoded = serde_json::to_vec(journal.entries()).expect("journal transport bytes");
    let decoded = serde_json::from_slice(&encoded).expect("journal transport decode");
    let rebuilt = InMemoryFederationJournal::from_entries(decoded).expect("rebuild journal");
    assert_eq!(rebuilt.latest_order_digest(), journal.latest_order_digest());
    assert_eq!(rebuilt.len(), 1);
}

#[test]
fn established_wire_replays_with_stable_conflicts_and_materialized_keys() {
    let envelope = envelope(receive_intent());
    let operation_id = envelope.operation_id().expect("operation ID");
    let wire = serde_json::to_value(&envelope).expect("established envelope wire");
    assert_eq!(
        wire.get("version").and_then(serde_json::Value::as_u64),
        Some(6)
    );
    assert_eq!(
        wire.pointer("/operation/type")
            .and_then(serde_json::Value::as_str),
        Some("bark")
    );

    let decoded: FederationOperationEnvelope =
        serde_json::from_value(wire.clone()).expect("established envelope decode");
    assert_eq!(
        decoded.operation_id().expect("decoded operation ID"),
        operation_id
    );
    assert_eq!(
        serde_json::to_value(&decoded).expect("re-encoded envelope"),
        wire
    );

    let mut journal = InMemoryFederationJournal::new();
    let entry = match journal
        .append(decoded)
        .expect("append established operation")
    {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => entry,
    };
    assert!(entry.conflict_keys.iter().all(|key| {
        serde_json::to_value(key)
            .expect("conflict key")
            .get("type")
            .and_then(serde_json::Value::as_str)
            == Some("bark")
    }));

    let materialized = entry
        .materialized_state_keys()
        .expect("established materialized-state keys");
    let kinds = materialized
        .iter()
        .map(|record| record.kind_label())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        kinds,
        std::collections::BTreeSet::from(["bark_action", "bark_derivation_allocation"])
    );
    let materialized_wire = serde_json::to_vec(&materialized).expect("materialized key wire");
    let decoded_materialized: Vec<cdk_common::federation::FederationMaterializedStateKeyRecord> =
        serde_json::from_slice(&materialized_wire).expect("established materialized key decode");
    assert_eq!(materialized, decoded_materialized);
}

#[test]
fn prepared_result_lookup_is_stable_across_journal_roundtrip() {
    let mut journal = InMemoryFederationJournal::new();
    journal
        .append(envelope(receive_intent()))
        .expect("append intent");
    journal
        .append(envelope(receive_prepared()))
        .expect("append prepared result");
    let record = FederationBarkOperationRecord::latest_for_action_phase(
        journal.entries(),
        action(1),
        FederationBarkSemanticPhase::ReceivePrepared,
    )
    .expect("prepared result");
    assert_eq!(record.operation_index, 1);
    assert_eq!(record.status, FederationOperationStatus::Accepted);
    assert_eq!(record.result_hash, Some(digest(16)));

    let rebuilt = InMemoryFederationJournal::from_entries(journal.entries().to_vec())
        .expect("rebuild result journal");
    assert_eq!(
        FederationBarkOperationRecord::latest_for_action_phase(
            rebuilt.entries(),
            action(1),
            FederationBarkSemanticPhase::ReceivePrepared,
        ),
        Some(record)
    );
}

#[test]
fn typed_conflicts_cover_quote_hash_vtxo_locator_phase_and_attempt() {
    let mut journal = InMemoryFederationJournal::new();
    journal
        .append(envelope(receive_intent()))
        .expect("append receive intent");
    let FederationBarkOperation::ReceiveIntent(base) = receive_intent() else {
        panic!("receive intent")
    };
    let cases = [
        {
            let mut changed = base.clone();
            changed.action_id = action(30);
            changed.payment_hash = digest(31);
            changed.destination = FederationBarkVtxoLocator::vtxo_custody(30);
            FederationBarkOperation::ReceiveIntent(changed)
        },
        {
            let mut changed = base.clone();
            changed.action_id = action(31);
            changed.quote = "other-quote".to_string();
            changed.destination = FederationBarkVtxoLocator::vtxo_custody(31);
            FederationBarkOperation::ReceiveIntent(changed)
        },
        {
            let mut changed = base.clone();
            changed.action_id = action(32);
            changed.quote = "other-quote-2".to_string();
            changed.payment_hash = digest(32);
            FederationBarkOperation::ReceiveIntent(changed)
        },
        {
            let mut changed = base;
            changed.quote = "other-quote-3".to_string();
            changed.payment_hash = digest(33);
            changed.destination = FederationBarkVtxoLocator::vtxo_custody(33);
            FederationBarkOperation::ReceiveIntent(changed)
        },
    ];
    for operation in cases {
        assert!(journal.append(envelope(operation)).is_err());
    }

    let plan = send_plan();
    let plan_keys = plan.conflict_keys();
    assert!(plan_keys
        .iter()
        .any(|key| matches!(key, FederationBarkConflictKey::VtxoReservation { .. })));
    assert!(plan_keys
        .iter()
        .any(|key| matches!(key, FederationBarkConflictKey::Locator { .. })));
    assert!(plan_keys.iter().any(|key| matches!(
        key,
        FederationBarkConflictKey::ActionPhase {
            attempt: Some(1),
            ..
        }
    )));
    assert!(plan_keys.iter().any(|key| matches!(
        key,
        FederationBarkConflictKey::SigningAttempt { attempt: 1, .. }
    )));
    let mut plan_journal = InMemoryFederationJournal::new();
    plan_journal
        .append(envelope(plan.clone()))
        .expect("append plan");
    let FederationBarkOperation::ActionPlan(base) = plan else {
        panic!("action plan")
    };
    let mut vtxo_conflict = base.clone();
    let invoice = vtxo_conflict
        .lightning_send_invoice()
        .expect("conflicting plan invoice");
    vtxo_conflict.action_id = action(40);
    vtxo_conflict.quote = Some("melt-quote-2".to_string());
    vtxo_conflict.outputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(40);
    vtxo_conflict.predecessor_operation_id = operation_id(40);
    vtxo_conflict.external_id = FederationBarkExternalId::derive_lightning_send(
        vtxo_conflict.action_id,
        vtxo_conflict.predecessor_operation_id,
        vtxo_conflict.attempt,
    )
    .expect("conflicting external ID");
    vtxo_conflict
        .bind_lightning_send_artifacts(&invoice)
        .expect("bind conflicting plan");
    assert!(plan_journal
        .append(envelope(FederationBarkOperation::ActionPlan(vtxo_conflict)))
        .is_err());

    let mut attempt_conflict = base;
    attempt_conflict.fee_bound = Amount::from(9);
    let invoice = attempt_conflict
        .lightning_send_invoice()
        .expect("attempt-conflicting plan invoice");
    attempt_conflict
        .bind_lightning_send_artifacts(&invoice)
        .expect("rebind attempt-conflicting plan");
    assert!(plan_journal
        .append(envelope(FederationBarkOperation::ActionPlan(
            attempt_conflict
        )))
        .is_err());
}

#[test]
fn predecessor_conflicts_are_scoped_to_the_action_lineage() {
    let first = refresh_plan();
    let mut second = first.clone();
    let predecessor = second.predecessor_operation_id;
    second.action_id = action(70);
    second.inputs[0].vtxo_id = vtxo(70);
    second.inputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(20);
    second.outputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(21);
    second.external_id =
        FederationBarkExternalId::derive_refresh(second.action_id, predecessor, second.attempt)
            .expect("second refresh external id");
    second.artifacts.clear();
    second
        .bind_refresh_artifacts(800, 900, [maintenance_input_artifact()])
        .expect("bind second refresh request");

    let mut journal = InMemoryFederationJournal::new();
    journal
        .append(envelope(FederationBarkOperation::ActionPlan(first)))
        .expect("first action");
    journal
        .append(envelope(FederationBarkOperation::ActionPlan(second)))
        .expect("different action may reuse the same predecessor");

    assert_eq!(journal.entries().len(), 2);
    let bark_key = FederationBarkConflictKey::Predecessor {
        action_id: action(70),
        operation_id: predecessor,
    };
    let expected_key = serde_json::json!({
        "type": "bark",
        "data": { "key": bark_key },
    });
    assert!(journal.entries()[1]
        .conflict_keys
        .iter()
        .any(|key| { serde_json::to_value(key).expect("conflict-key JSON") == expected_key }));
}

#[test]
fn predecessor_conflicts_still_reject_same_lineage_forks() {
    let plan = refresh_plan();
    let plan_operation_id = operation_id(71);
    let predecessor = operation_id(72);
    let first = FederationBarkActionTransition::signing_attempt_advanced(
        plan_operation_id,
        predecessor,
        &plan,
        plan.actor,
        2,
        vec![member(2), member(3)],
    )
    .expect("first retry transition");
    let second = FederationBarkActionTransition::signing_attempt_advanced(
        plan_operation_id,
        predecessor,
        &plan,
        plan.actor,
        3,
        vec![member(3), member(4)],
    )
    .expect("competing retry transition");
    let mut journal = InMemoryFederationJournal::new();
    journal
        .append(envelope(FederationBarkOperation::ActionTransition(first)))
        .expect("first lineage edge");

    assert!(journal
        .append(envelope(FederationBarkOperation::ActionTransition(second)))
        .is_err());
}

#[test]
fn accepted_actor_replacement_preserves_phase_and_changes_only_executor_attempt() {
    let plan = refresh_plan();
    let mut journal = InMemoryFederationJournal::new();
    let plan_entry = match journal
        .append(envelope(FederationBarkOperation::ActionPlan(plan.clone())))
        .expect("append refresh plan")
    {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => entry,
    };
    let action = crate::FederationBarkActionRecord::new(
        &plan_entry,
        &FederationBarkOperation::ActionPlan(plan.clone()),
    )
    .expect("initial action projection");
    let retry = FederationBarkActionTransition::signing_attempt_advanced(
        plan_entry.operation_id,
        action.latest_operation_id,
        &plan,
        member(2),
        2,
        vec![member(2), member(3)],
    )
    .expect("actor replacement");
    let retry_operation = FederationBarkOperation::ActionTransition(retry);
    let retry_entry = match journal
        .append(envelope(retry_operation.clone()))
        .expect("append actor replacement")
    {
        FederationJournalAppendOutcome::Appended(entry)
        | FederationJournalAppendOutcome::Duplicate(entry) => entry,
    };
    let advanced = action
        .advanced(&retry_entry, &retry_operation)
        .expect("advance action projection");

    assert_eq!(advanced.actor, member(2));
    assert_eq!(advanced.attempt, 2);
    assert_eq!(
        advanced.signing_retry_phase,
        Some(FederationBarkSemanticPhase::RefreshPlanned)
    );
    assert_eq!(advanced.plan_operation_id, Some(plan_entry.operation_id));
    assert_eq!(advanced.action_id, plan.action_id);
}

#[test]
fn author_and_balance_mutations_fail_closed() {
    let FederationBarkOperation::ReceiveIntent(mut intent) = receive_intent() else {
        panic!("receive intent")
    };
    intent.actor = member(3);
    let operation = FederationBarkOperation::ReceiveIntent(intent);
    assert_eq!(operation.claimed_author(), Some(member(3)));
    assert!(operation.validate().is_err());

    let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
        panic!("action plan")
    };
    plan.fee_amount = Amount::from(6);
    assert!(FederationBarkOperation::ActionPlan(plan)
        .validate()
        .is_err());
}

#[test]
fn objective_payment_success_can_be_published_by_an_authenticated_fallback() {
    let FederationBarkOperation::ActionPlan(plan) = send_plan() else {
        panic!("action plan")
    };
    let mut transition = FederationBarkActionTransition::signing_attempt_advanced(
        operation_id(91),
        operation_id(92),
        &plan,
        plan.actor,
        2,
        vec![member(2), member(3)],
    )
    .expect("transition fixture");

    transition.phase = FederationBarkSemanticPhase::PaymentSucceeded;
    assert!(FederationBarkOperation::ActionTransition(transition.clone()).preserves_action_actor());
    assert_eq!(
        FederationBarkOperation::ActionTransition(transition.clone()).claimed_author(),
        None,
    );

    transition.phase = FederationBarkSemanticPhase::PaymentFailed;
    assert!(
        !FederationBarkOperation::ActionTransition(transition.clone()).preserves_action_actor()
    );
    assert_eq!(
        FederationBarkOperation::ActionTransition(transition).claimed_author(),
        Some(plan.actor),
    );
}

#[test]
fn lightning_send_binds_inputs_keys_fee_expiry_and_invoice() {
    let FederationBarkOperation::ActionPlan(base) = send_plan() else {
        panic!("action plan")
    };
    let mut mutations = Vec::new();
    let mut changed = base.clone();
    changed.inputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(40);
    mutations.push(changed);
    let mut changed = base.clone();
    changed.outputs[0].locator = FederationBarkVtxoLocator::vtxo_custody(41);
    mutations.push(changed);
    let mut changed = base.clone();
    changed.revocation_locator = Some(FederationBarkVtxoLocator::vtxo_custody(42));
    mutations.push(changed);
    let mut changed = base.clone();
    changed.fee_bound = Amount::from(9);
    mutations.push(changed);
    let mut changed = base.clone();
    changed.expires_at += 1;
    mutations.push(changed);
    let mut changed = base;
    let invoice = changed
        .artifacts
        .iter_mut()
        .find(|artifact| artifact.kind == FederationBarkArtifactKind::Invoice)
        .expect("invoice artifact");
    invoice.bytes.push(b' ');
    invoice.hash =
        FederationBarkDigest::from_bytes(Sha256Hash::hash(&invoice.bytes).to_byte_array())
            .expect("changed invoice hash");
    changed.artifacts.sort();
    mutations.push(changed);

    for plan in mutations {
        assert!(FederationBarkOperation::ActionPlan(plan)
            .validate()
            .is_err());
    }
}

#[test]
fn failed_send_revocation_reuses_only_the_preallocated_recovery_locator() {
    let FederationBarkOperation::ActionPlan(send) = send_plan() else {
        panic!("action plan")
    };
    let predecessor = operation_id(50);
    let input_artifacts = [
        b"failed split HTLC 1".to_vec(),
        b"failed split HTLC 2".to_vec(),
        b"failed split HTLC 3".to_vec(),
    ]
    .into_iter()
    .map(|bytes| bark_artifact(FederationBarkArtifactKind::Vtxo, bytes))
    .collect::<Result<Vec<_>>>()
    .expect("HTLC artifacts");
    let request_artifact = bark_artifact(
        FederationBarkArtifactKind::ArkRequest,
        b"accepted revocation request".to_vec(),
    )
    .expect("request artifact");
    let mut artifacts = input_artifacts;
    artifacts.push(request_artifact.clone());
    artifacts.sort();
    let revocation_locator = send.revocation_locator.expect("revocation locator");
    let htlc_locator = send.outputs[0].locator;
    let plan = FederationBarkActionPlan {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: send.action_id,
        quote: send.quote,
        kind: FederationBarkActionKind::FailedSendRevocation,
        actor: send.actor,
        predecessor_operation_id: predecessor,
        payment_hash: send.payment_hash,
        inputs: vec![
            FederationBarkVtxoInput {
                vtxo_id: vtxo(51),
                locator: htlc_locator,
                amount: Amount::from(201),
            },
            FederationBarkVtxoInput {
                vtxo_id: vtxo(52),
                locator: htlc_locator,
                amount: Amount::from(250),
            },
            FederationBarkVtxoInput {
                vtxo_id: vtxo(53),
                locator: htlc_locator,
                amount: Amount::from(574),
            },
        ],
        outputs: vec![FederationBarkVtxoOutput {
            locator: revocation_locator,
            amount: Amount::from(1_025),
            artifact_hash: request_artifact.hash,
        }],
        revocation_locator: None,
        external_amount: Amount::ZERO,
        fee_amount: Amount::ZERO,
        fee_bound: Amount::ZERO,
        certified_receive_htlcs: None,
        artifacts,
        external_id: FederationBarkExternalId::derive_failed_send_revocation(
            send.action_id,
            predecessor,
            send.attempt,
        )
        .expect("revocation external id"),
        signer_set: send.signer_set,
        attempt: send.attempt,
        expires_at: send.expires_at,
    };
    let operation = FederationBarkOperation::ActionPlan(plan.clone());
    operation.validate().expect("valid revocation plan");
    assert!(operation
        .conflict_keys()
        .iter()
        .all(|key| !matches!(key, FederationBarkConflictKey::VtxoReservation { .. })));
    assert!(operation.conflict_keys().iter().all(|key| {
        !matches!(
            key,
            FederationBarkConflictKey::Locator { locator }
                if *locator == revocation_locator
        )
    }));

    let mut changed = plan.clone();
    changed.outputs[0].locator = changed.inputs[0].locator;
    assert!(FederationBarkOperation::ActionPlan(changed)
        .validate()
        .is_err());
    let mut changed = plan;
    changed.outputs[0].amount = Amount::from(1_024);
    assert!(FederationBarkOperation::ActionPlan(changed)
        .validate()
        .is_err());
}

#[test]
fn outgoing_fee_bound_rounds_up_conservatively() {
    let policy = FederationBarkPolicyConfig::new(
        0,
        0,
        10,
        5_000,
        18,
        18,
        144,
        18,
        30,
        FederationBarkReceiveAntiDosPolicy::NoProof,
    )
    .expect("policy");
    assert_eq!(
        policy.maximum_outgoing_fee(Amount::from(1)).expect("fee"),
        Amount::from(11),
    );
    assert_eq!(
        policy
            .maximum_outgoing_fee(Amount::from(1_000))
            .expect("fee"),
        Amount::from(15),
    );
}

#[test]
fn collection_artifact_and_invoice_bounds_fail_closed() {
    let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
        panic!("action plan")
    };
    plan.inputs = (1..=65)
        .map(|seed| FederationBarkVtxoInput {
            vtxo_id: vtxo(seed),
            locator: FederationBarkVtxoLocator::vtxo_custody(u32::from(seed)),
            amount: Amount::from(1),
        })
        .collect();
    assert!(FederationBarkOperation::ActionPlan(plan)
        .validate()
        .is_err());

    let FederationBarkOperation::ActionPlan(mut plan) = send_plan() else {
        panic!("action plan")
    };
    let bytes = vec![1; FEDERATION_MAX_BARK_ARTIFACT_BYTES + 1];
    plan.artifacts = vec![FederationBarkArtifact {
        kind: FederationBarkArtifactKind::Vtxo,
        hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())
            .expect("artifact hash"),
        bytes,
    }];
    assert!(FederationBarkOperation::ActionPlan(plan)
        .validate()
        .is_err());

    let FederationBarkOperation::ReceivePrepared(mut prepared) = receive_prepared() else {
        panic!("receive prepared")
    };
    prepared.invoice = "x".repeat(FEDERATION_MAX_BARK_INVOICE_BYTES + 1);
    assert!(FederationBarkOperation::ReceivePrepared(prepared)
        .validate()
        .is_err());
}

#[test]
fn authorization_slot_excludes_attempt_but_session_does_not() {
    let application_id = FederationFrostApplicationId::from_bytes([7; 32]).expect("application ID");
    let reference = FederationBarkAuthorizationReference {
        version: FEDERATION_BARK_AUTHORIZATION_VERSION,
        federation_id: federation_id(),
        application_id,
        root_key_epoch: 1,
        application_key_epoch: 1,
        application_derivation_version: 1,
        accepted_operation_id: operation_id(8),
        accepted_operation_index: 9,
        action_id: action(10),
        phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
        object_ordinal: 0,
        operation_kind: FederationBarkOperationKind::ActionTransition,
        plan_or_result_hash: digest(11),
        locator: FederationBarkVtxoLocator::vtxo_custody(12),
        protocol: FederationBarkCustodyProtocol::Musig2,
        transcript_hash: digest(13),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        network: Network::Regtest,
        ark_server_public_key: ark_server_key(),
    };
    reference.validate().expect("valid authorization");
    let slot = reference.signing_slot().expect("slot");
    let session = reference.session_id().expect("session");
    let mut retry = reference.clone();
    retry.signer_set = vec![member(2), member(3)];
    retry.attempt = 2;
    assert_eq!(retry.signing_slot().expect("retry slot"), slot);
    assert_ne!(retry.session_id().expect("retry session"), session);
    let mut mutated = reference;
    mutated.transcript_hash = digest(14);
    assert_ne!(mutated.signing_slot().expect("mutated slot"), slot);
    let mut incompatible = mutated;
    incompatible.operation_kind = FederationBarkOperationKind::ReceiveIntent;
    assert!(incompatible.validate().is_err());
}

#[test]
fn lightning_send_expiry_is_bounded_from_accepted_inputs() {
    validate_federation_bark_lightning_send_expiry(314_076, 314_279)
        .expect("current Ark expiry is bounded");
    validate_federation_bark_lightning_send_expiry(314_076, 314_075)
        .expect("the Ark server owns the live lower-bound check");
    assert!(validate_federation_bark_lightning_send_expiry(0, 1).is_err());
    assert!(validate_federation_bark_lightning_send_expiry(1, 0).is_err());
    assert!(validate_federation_bark_lightning_send_expiry(1, 2_018).is_err());
}

#[test]
fn frost_authorization_binds_exact_canonical_transcript() {
    let transcript = vec![0xab; 12_000];
    let application_id = FederationFrostApplicationId::from_bytes([7; 32]).expect("application ID");
    let reference = FederationBarkAuthorizationReference {
        version: FEDERATION_BARK_AUTHORIZATION_VERSION,
        federation_id: federation_id(),
        application_id,
        root_key_epoch: 1,
        application_key_epoch: 1,
        application_derivation_version: 1,
        accepted_operation_id: operation_id(8),
        accepted_operation_index: 9,
        action_id: action(10),
        phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
        object_ordinal: 0,
        operation_kind: FederationBarkOperationKind::ActionTransition,
        plan_or_result_hash: digest(11),
        locator: FederationBarkVtxoLocator::vtxo_custody(12),
        protocol: FederationBarkCustodyProtocol::Musig2,
        transcript_hash: federation_bark_custody_transcript_hash(&transcript)
            .expect("transcript hash"),
        signer_set: vec![member(1), member(2)],
        attempt: 1,
        network: Network::Regtest,
        ark_server_public_key: ark_server_key(),
    };
    let authorization = FederationBarkFrostAuthorization::new(
        reference,
        [42; 32],
        &cdk_frost::SigningDerivation::application_key(),
        transcript,
    )
    .expect("valid exact authorization");
    let canonical = authorization.canonical_bytes().expect("canonical bytes");
    assert!(canonical.len() <= FEDERATION_MAX_BARK_AUTHORIZATION_BYTES);
    assert_eq!(
        FederationBarkFrostAuthorization::from_bytes(&canonical).expect("canonical round trip"),
        authorization
    );

    let mut changed_transcript = authorization.clone();
    changed_transcript.request_transcript.push(0);
    assert!(changed_transcript.validate().is_err());

    let mut empty_message = authorization.clone();
    empty_message.message = [0; 32];
    assert!(empty_message.validate().is_err());

    let mut noncanonical = b" ".to_vec();
    noncanonical.extend_from_slice(&canonical);
    assert!(FederationBarkFrostAuthorization::from_bytes(&noncanonical).is_err());
}

#[test]
fn action_id_derivation_has_a_fixed_vector() {
    let action_id = FederationBarkActionId::derive(
        federation_id(),
        operation_id(8),
        FederationBarkSemanticPhase::SendHtlcsCosigned,
        3,
    )
    .expect("action ID");
    assert_eq!(
        action_id.to_hex(),
        "2007f3f7f11ea3e128c6470494df3e6b3dee0791d946e79ff396b148ecf6123d"
    );

    let changed = FederationBarkActionId::derive(
        federation_id(),
        operation_id(8),
        FederationBarkSemanticPhase::SendHtlcsCosigned,
        4,
    )
    .expect("changed action ID");
    assert_ne!(changed, action_id);
}

#[test]
fn incoming_claim_requires_current_certified_evidence() {
    let mut plan = certified_receive_claim_plan();
    plan.certified_receive_htlcs = None;
    assert!(FederationBarkOperation::ActionPlan(plan)
        .validate()
        .is_err());
}
