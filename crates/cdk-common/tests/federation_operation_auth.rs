#![cfg(feature = "federation")]

//! Regression tests for durable operation authorship at the consensus boundary.

use std::str::FromStr;

use cdk_common::federation::{
    generate_trusted_dealer_configs, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftUnitPayload, FederationConsensusItem, FederationConsensusRuntime,
    FederationError, FederationMintQuotePaymentObservation, FederationOperation,
    FederationOperationEnvelope, FederationOperationId, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerMember,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{Amount, MintQuoteState, PaymentMethod, SecretKey};

fn member_id(id: u16) -> MemberId {
    MemberId::new(id).expect("member id")
}

fn config_bundle() -> TrustedDealerConfigBundle {
    let members = (1..=3)
        .map(|id| {
            TrustedDealerMember::new(
                member_id(id),
                MintUrl::from_str(&format!("https://mint{id}.example.com"))
                    .expect("public mint URL"),
                MintUrl::from_str(&format!("https://fed{id}.example.com"))
                    .expect("federation API URL"),
                SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
            )
        })
        .collect();
    generate_trusted_dealer_configs(
        ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
        members,
        Vec::new(),
    )
    .expect("trusted dealer config")
}

fn payment_observation(observer: MemberId) -> FederationOperation {
    FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
        quote: "quote-auth".to_string(),
        observer,
        method: PaymentMethod::BOLT11,
        state: MintQuoteState::Paid,
        amount_paid: Amount::from(2),
        amount_issued: Amount::ZERO,
        payment_id: Some(PaymentIdentifier::CustomId("payment-auth".to_string())),
        payment_proof: Some("settled".to_string()),
    })
}

fn payment_record(
    index: u64,
    observer: MemberId,
    state: MintQuoteState,
    proof: &str,
) -> FederationPaymentObservationRecord {
    FederationPaymentObservationRecord {
        operation_id: FederationOperationId::from_bytes([index as u8 + 1; 32]),
        operation_index: index,
        observation: FederationPaymentObservation::MintQuote(
            FederationMintQuotePaymentObservation {
                quote: "quote-policy".to_string(),
                observer,
                method: PaymentMethod::BOLT11,
                state,
                amount_paid: Amount::from(2),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some(proof.to_string()),
            },
        ),
    }
}

#[test]
fn authorization_binds_the_original_member_and_payload() {
    let bundle = config_bundle();
    let private_member = bundle
        .private_configs
        .get(&member_id(1))
        .expect("member private config");
    let unsigned = FederationOperationEnvelope::new(
        bundle.public_config.federation_id,
        payment_observation(member_id(1)),
    );
    let operation_id = unsigned.operation_id().expect("unsigned operation id");
    assert!(matches!(
        unsigned.verify_authorization(&bundle.public_config),
        Err(FederationError::OperationAuthorizationMissing)
    ));

    let signed = unsigned
        .authorize(private_member)
        .expect("authorize operation");
    assert_eq!(
        signed
            .verify_authorization(&bundle.public_config)
            .expect("verify author"),
        member_id(1)
    );
    assert_eq!(
        signed.operation_id().expect("signed operation id"),
        operation_id
    );

    let mut tampered = signed;
    let FederationOperation::MintQuotePayment(observation) = &mut tampered.operation else {
        panic!("mint payment observation");
    };
    observation.amount_paid = Amount::from(3);
    assert!(matches!(
        tampered.verify_authorization(&bundle.public_config),
        Err(FederationError::OperationSignatureVerificationFailed { member_id: author })
            if author == member_id(1)
    ));
}

#[test]
fn authenticated_consensus_rejects_observer_impersonation_without_poisoning_the_operation_id() {
    let bundle = config_bundle();
    let config = &bundle.public_config;
    let mut runtime = FederationConsensusRuntime::authenticated_aleph_bft_from_finalized_items(
        config,
        member_id(1),
        0,
        Vec::new(),
    )
    .expect("authenticated runtime");
    let impersonation =
        FederationOperationEnvelope::new(config.federation_id, payment_observation(member_id(2)))
            .authorize(
                bundle
                    .private_configs
                    .get(&member_id(1))
                    .expect("first member private config"),
            )
            .expect("authorize impersonation");
    let operation_id = impersonation.operation_id().expect("operation id");
    assert!(matches!(
        impersonation.verify_authorization(config),
        Err(FederationError::OperationAuthorMismatch {
            author,
            claimed_member,
        }) if author == member_id(1) && claimed_member == member_id(2)
    ));
    let invalid_unit = FederationAlephBftUnitPayload::new(
        config.federation_id,
        member_id(1),
        0,
        0,
        vec![FederationConsensusItem::operation(impersonation)],
    )
    .expect("invalid unit");

    assert!(runtime
        .finalize_unit(invalid_unit)
        .expect("reject invalid operation without halting")
        .is_empty());
    assert!(runtime.journal().entries().is_empty());
    assert_eq!(
        runtime.finalized_items()[0].status,
        FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
    );
    runtime = FederationConsensusRuntime::authenticated_aleph_bft_from_finalized_items(
        config,
        member_id(1),
        0,
        runtime.finalized_items().to_vec(),
    )
    .expect("recover rejected operation");
    assert!(runtime.journal().entries().is_empty());

    let valid =
        FederationOperationEnvelope::new(config.federation_id, payment_observation(member_id(2)))
            .authorize(
                bundle
                    .private_configs
                    .get(&member_id(2))
                    .expect("second member private config"),
            )
            .expect("authorize valid observation");
    assert_eq!(
        valid.operation_id().expect("valid operation id"),
        operation_id
    );
    let valid_unit = FederationAlephBftUnitPayload::new(
        config.federation_id,
        member_id(2),
        0,
        0,
        vec![FederationConsensusItem::operation(valid)],
    )
    .expect("valid unit");

    assert_eq!(
        runtime
            .finalize_unit(valid_unit)
            .expect("accept valid operation")
            .len(),
        1
    );
    assert_eq!(runtime.journal().entries().len(), 1);
    assert_eq!(
        runtime.finalized_items()[1].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
}

#[test]
fn minority_disagreement_does_not_veto_a_payment_threshold() {
    let records = vec![
        payment_record(0, member_id(1), MintQuoteState::Paid, "settled"),
        payment_record(1, member_id(3), MintQuoteState::Paid, "different"),
        payment_record(2, member_id(2), MintQuoteState::Paid, "settled"),
    ];
    let report = FederationPaymentObservationReport::from_records(
        FederationPaymentObservationContext::MintQuote,
        "quote-policy",
        2,
        &records,
    );

    assert!(!report.has_competing_quorums());
    assert!(!report.is_unresolved());
}

#[test]
fn incompatible_payment_thresholds_leave_the_quote_unresolved() {
    let records = vec![
        payment_record(0, member_id(1), MintQuoteState::Paid, "settled"),
        payment_record(1, member_id(2), MintQuoteState::Paid, "settled"),
        payment_record(2, member_id(2), MintQuoteState::Issued, "issued"),
        payment_record(3, member_id(3), MintQuoteState::Issued, "issued"),
    ];
    let report = FederationPaymentObservationReport::from_records(
        FederationPaymentObservationContext::MintQuote,
        "quote-policy",
        2,
        &records,
    );

    assert!(report.has_competing_quorums());
    assert!(report.is_unresolved());
}
