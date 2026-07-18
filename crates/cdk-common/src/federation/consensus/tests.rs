use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use serde_json::json;

use super::super::{
    FederationConfig, FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate,
    MemberConfigPublic, ThresholdParams,
};
use super::*;
use crate::mint_url::MintUrl;
use crate::{
    Amount, BlindedMessage, CurrencyUnit, Id, MeltQuoteBolt11Request, MeltQuoteRequest,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteRequest, MintQuoteResponse,
    MintRequest, PublicKey, SecretKey,
};

fn federation_id(byte: u8) -> FederationId {
    FederationId::from_bytes([byte; 32])
}

fn member_id() -> MemberId {
    MemberId::new(1).expect("member id")
}

fn member_id_value(id: u16) -> MemberId {
    MemberId::new(id).expect("member id")
}

fn recover_raw_for_test(
    federation_id: FederationId,
    local_member: MemberId,
    session: u64,
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
) -> Result<FederationConsensusRuntime> {
    FederationConsensusRuntime::recover_raw_for_test(
        federation_id,
        local_member,
        session,
        None,
        finalized_items,
    )
}

fn recover_for_members_for_test<I>(
    federation_id: FederationId,
    local_member: MemberId,
    session: u64,
    allowed_members: I,
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
) -> Result<FederationConsensusRuntime>
where
    I: IntoIterator<Item = MemberId>,
{
    FederationConsensusRuntime::recover_raw_for_test(
        federation_id,
        local_member,
        session,
        Some(allowed_members.into_iter().collect::<BTreeSet<_>>()),
        finalized_items,
    )
}

fn recover_with_roster_for_test<I>(
    federation_id: FederationId,
    local_member: MemberId,
    session: u64,
    member_ids: I,
    consensus_threshold: u16,
    finalized_items: Vec<FederationAlephBftFinalizedItem>,
) -> Result<FederationConsensusRuntime>
where
    I: IntoIterator<Item = MemberId>,
{
    let allowed_members = validate_runtime_roster(member_ids, consensus_threshold, local_member)?;
    FederationConsensusRuntime::recover_raw_for_test(
        federation_id,
        local_member,
        session,
        Some(allowed_members),
        finalized_items,
    )
}

fn keyset_id() -> Id {
    Id::from_str("00bfa73302d12ffd").expect("keyset id")
}

fn output(hex: &str) -> BlindedMessage {
    BlindedMessage::new(
        Amount::from(1),
        keyset_id(),
        PublicKey::from_hex(hex).expect("public key"),
    )
}

fn mint_envelope(
    federation_id: FederationId,
    quote: &str,
    hex: &str,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        super::super::FederationOperation::Mint(super::super::FederationMintOperation {
            request: super::super::FederationMintRequest::Single(MintRequest {
                quote: quote.to_string(),
                outputs: vec![output(hex)],
                signature: None,
            }),
        }),
    )
}

fn mint_quote_envelope_with_amount(
    federation_id: FederationId,
    quote: &str,
    amount: u64,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        super::super::FederationOperation::MintQuote(super::super::FederationMintQuoteOperation {
            quote: quote.to_string(),
            request_lookup_id: crate::payment::PaymentIdentifier::CustomId(quote.to_string()),
            request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(amount),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: quote.to_string(),
                request: format!("invoice-{quote}-{amount}"),
                amount: Some(Amount::from(amount)),
                unit: Some(CurrencyUnit::Sat),
                method: crate::PaymentMethod::BOLT11,
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                updated_at: 0,
                state: crate::MintQuoteState::Unpaid,
                expiry: None,
                pubkey: None,
            }),
        }),
    )
}

fn melt_quote_envelope(
    federation_id: FederationId,
    quote: &str,
    unit: CurrencyUnit,
) -> FederationOperationEnvelope {
    let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq"
            .parse()
            .expect("bolt11 invoice"),
        unit,
        options: None,
    });
    FederationOperationEnvelope::new(
        federation_id,
        super::super::FederationOperation::MeltQuote(
            super::super::operation::test_melt_quote_operation(quote, request),
        ),
    )
}

fn module_fact_item(kind: &str) -> FederationConsensusItem {
    FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new("test", kind, json!({})))
}

fn recovery_barrier(
    federation_id: FederationId,
    requested_after: u64,
) -> FederationRecoveryBarrier {
    FederationRecoveryBarrier::new(federation_id, 0, None, requested_after)
        .expect("recovery barrier")
}

fn identity_secret(id: u8) -> SecretKey {
    SecretKey::from_slice(&[id; 32]).expect("identity secret")
}

fn consensus_member_config(id: u16, identity_secret: &SecretKey) -> MemberConfigPublic {
    MemberConfigPublic {
        member_id: member_id_value(id),
        public_mint_url: MintUrl::from_str(&format!("https://mint{id}.example.com"))
            .expect("public mint url"),
        federation_api_url: MintUrl::from_str(&format!("https://fed{id}.example.com"))
            .expect("federation api url"),
        identity_public_key: identity_secret.public_key(),
    }
}

fn private_member(id: u16, identity_secret: SecretKey) -> MemberConfigPrivate {
    MemberConfigPrivate {
        federation_id: None,
        member_id: member_id_value(id),
        identity_secret_key: identity_secret,
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    }
}

fn consensus_config_three() -> (FederationConfig, Vec<MemberConfigPrivate>) {
    let first_secret = identity_secret(1);
    let second_secret = identity_secret(2);
    let third_secret = identity_secret(3);
    let members = BTreeMap::from([
        (
            member_id_value(1),
            consensus_member_config(1, &first_secret),
        ),
        (
            member_id_value(2),
            consensus_member_config(2, &second_secret),
        ),
        (
            member_id_value(3),
            consensus_member_config(3, &third_secret),
        ),
    ]);
    let threshold_params = ThresholdParams::new(3, 2, 2).expect("threshold params");
    let config =
        FederationConfig::new(threshold_params, members, Vec::new()).expect("federation config");
    let private_members = vec![
        private_member(1, first_secret),
        private_member(2, second_secret),
        private_member(3, third_secret),
    ];
    (config, private_members)
}

fn member_roster(count: u16) -> Vec<MemberId> {
    (1..=count).map(member_id_value).collect()
}

fn aleph_adapters_for_roster(
    federation_id: FederationId,
    members: &[MemberId],
) -> Vec<FederationConsensusRuntime> {
    members
        .iter()
        .map(|member| {
            FederationConsensusRuntime::for_members(
                federation_id,
                *member,
                0,
                members.iter().copied(),
            )
            .expect("roster-bound AlephBFT adapter")
        })
        .collect()
}

fn aleph_operation_unit(
    federation_id: FederationId,
    submitting_member: MemberId,
    session: u64,
    unit_index: u64,
    envelopes: Vec<FederationOperationEnvelope>,
) -> FederationAlephBftUnitPayload {
    FederationAlephBftUnitPayload::new(
        federation_id,
        submitting_member,
        session,
        unit_index,
        envelopes
            .into_iter()
            .map(FederationConsensusItem::operation)
            .collect(),
    )
    .expect("AlephBFT operation unit")
}

fn journal_fingerprint(
    adapter: &FederationConsensusRuntime,
) -> Vec<(u64, String, FederationOperationStatus, String, String)> {
    adapter
        .journal()
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.index,
                entry.operation_id.to_string(),
                entry.status(),
                entry.previous_order_digest.to_string(),
                entry.order_digest.to_string(),
            )
        })
        .collect()
}

type FinalizedItemFingerprint = (
    u64,
    u64,
    u64,
    u16,
    u64,
    u64,
    Option<String>,
    FederationAlephBftFinalizedItemStatus,
);

fn finalized_item_fingerprint(
    adapter: &FederationConsensusRuntime,
) -> Vec<FinalizedItemFingerprint> {
    adapter
        .finalized_items()
        .iter()
        .map(|item| {
            (
                item.accepted_item_index,
                item.session,
                item.unit_finalization_index,
                item.unit_submitting_member.get(),
                item.unit_index,
                item.item_index_in_unit,
                item.operation_id
                    .map(|operation_id| operation_id.to_string()),
                item.status,
            )
        })
        .collect()
}

fn assert_aleph_adapters_converged(adapters: &[FederationConsensusRuntime]) {
    let first = adapters.first().expect("at least one adapter");
    let expected_journal = journal_fingerprint(first);
    let expected_finalized = finalized_item_fingerprint(first);
    let expected_status = first.status();

    for adapter in adapters.iter().skip(1) {
        assert_eq!(journal_fingerprint(adapter), expected_journal);
        assert_eq!(finalized_item_fingerprint(adapter), expected_finalized);
        let status = adapter.status();
        assert_eq!(status.federation_id, expected_status.federation_id);
        assert_eq!(status.session, expected_status.session);
        assert_eq!(
            status.next_unit_finalization_index,
            expected_status.next_unit_finalization_index
        );
        assert_eq!(status.next_item_index, expected_status.next_item_index);
        assert_eq!(status.queued_item_count, expected_status.queued_item_count);
        assert_eq!(
            status.pending_finalization_item_count,
            expected_status.pending_finalization_item_count
        );
        assert_eq!(
            status.finalized_item_count,
            expected_status.finalized_item_count
        );
        assert_eq!(
            status.accepted_operation_count,
            expected_status.accepted_operation_count
        );
        assert_eq!(
            status.latest_accepted_operation_index,
            expected_status.latest_accepted_operation_index
        );
        assert_eq!(
            status.allowed_member_count,
            expected_status.allowed_member_count
        );
    }
}

#[test]
fn consensus_runtime_begin_aleph_bft_session_rotates_empty_session() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id(), 0);

    runtime
        .begin_aleph_bft_session(1)
        .expect("begin empty session");
    let status = runtime.aleph_bft_status().expect("status");
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(status.next_unit_finalization_index, 0);
    assert_eq!(status.next_item_index, 0);

    runtime
        .submit_aleph_bft_envelope(mint_envelope(
            federation_id,
            "quote-session-one",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit session-one operation");
    let unit = runtime
        .next_aleph_bft_unit_payload()
        .expect("next unit")
        .expect("unit present");
    assert_eq!(unit.session, 1);
    assert_eq!(unit.unit_index, 0);

    runtime
        .finalize_aleph_bft_unit(unit)
        .expect("finalize session-one unit");
    let finalized = runtime.aleph_bft_status().expect("finalized status");
    assert_eq!(finalized.session, 1);
    assert_eq!(finalized.next_unit_index, 1);
    assert_eq!(finalized.next_unit_finalization_index, 1);
    assert_eq!(finalized.next_item_index, 1);
}

#[test]
fn consensus_runtime_begin_aleph_bft_session_waits_for_pending_finality() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id(), 0);

    runtime
        .submit_aleph_bft_envelope(mint_envelope(
            federation_id,
            "quote-pending-session-zero",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit operation");
    let unit = runtime
        .next_aleph_bft_unit_payload()
        .expect("next unit")
        .expect("unit present");
    assert_eq!(
        runtime
            .aleph_bft_status()
            .expect("pending status")
            .pending_finalization_item_count,
        1
    );

    assert!(matches!(
        runtime.begin_aleph_bft_session(1),
        Err(FederationError::PendingAlephBftFinalization {
            current_session: 0,
            requested_session: 1,
            pending_units: 1,
            pending_items: 1,
        })
    ));
    assert_eq!(runtime.aleph_bft_status().expect("status").session, 0);

    runtime
        .finalize_aleph_bft_unit(unit)
        .expect("finalize pending unit");
    runtime
        .begin_aleph_bft_session(1)
        .expect("begin after finality");
    assert_eq!(runtime.aleph_bft_status().expect("rotated").session, 1);
}

#[test]
fn consensus_runtime_begin_aleph_bft_session_can_requeue_pending_finality() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id(), 0);

    let dispatched = mint_envelope(
        federation_id,
        "quote-requeued-session-zero",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let dispatched_operation_id = dispatched.operation_id().expect("dispatched operation id");
    runtime
        .submit_aleph_bft_envelope(dispatched)
        .expect("submit operation");
    let unit = runtime
        .next_aleph_bft_unit_payload()
        .expect("next unit")
        .expect("unit present");
    assert_eq!(unit.session, 0);
    assert_eq!(
        runtime
            .aleph_bft_status()
            .expect("pending status")
            .pending_finalization_item_count,
        1
    );
    let queued = mint_envelope(
        federation_id,
        "quote-queued-session-zero",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f42",
    );
    let queued_operation_id = queued.operation_id().expect("queued operation id");
    runtime
        .submit_aleph_bft_envelope(queued)
        .expect("submit queued operation");

    let requeued_items = runtime
        .begin_aleph_bft_session_requeue_pending_finalization(1)
        .expect("requeue pending finality into next session");
    assert_eq!(requeued_items, 1);
    let rotated = runtime.aleph_bft_status().expect("rotated status");
    assert_eq!(rotated.session, 1);
    assert_eq!(rotated.queued_item_count, 2);
    assert_eq!(rotated.pending_finalization_item_count, 0);

    let requeued_unit = runtime
        .next_aleph_bft_unit_payload()
        .expect("next requeued unit")
        .expect("requeued unit present");
    assert_eq!(requeued_unit.session, 1);
    assert_eq!(requeued_unit.unit_index, 0);
    assert_eq!(
        requeued_unit
            .operation_ids
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [dispatched_operation_id, queued_operation_id]
            .into_iter()
            .collect()
    );

    runtime
        .finalize_aleph_bft_unit(requeued_unit)
        .expect("finalize requeued unit");
    let finalized = runtime.aleph_bft_status().expect("finalized status");
    assert_eq!(finalized.session, 1);
    assert_eq!(finalized.queued_item_count, 0);
    assert_eq!(finalized.pending_finalization_item_count, 0);
    assert_eq!(finalized.next_item_index, 2);
}

#[test]
fn consensus_item_queue_batches_and_deduplicates_operations() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let mut queue = FederationConsensusItemQueue::new();

    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(envelope.clone()))
            .expect("submit operation"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(envelope.clone()))
            .expect("submit duplicate operation"),
        FederationConsensusItemAdmission::DuplicateOperation
    );
    assert_eq!(
        queue
            .submit(FederationConsensusItem::ModuleFact(
                FederationConsensusModuleFact::new("mint", "fact", json!({"ok": true}))
            ))
            .expect("submit module fact"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(queue.len(), 2);

    let first_batch = queue.drain_batch(1);
    assert_eq!(first_batch.len(), 1);
    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(envelope))
            .expect("resubmit drained operation"),
        FederationConsensusItemAdmission::Accepted
    );

    let second_batch = queue.drain_batch(10);
    assert_eq!(second_batch.len(), 2);
    assert!(queue.is_empty());
}

#[test]
fn recovery_barrier_generation_is_canonical_and_tamper_evident() {
    let federation_id = federation_id(31);
    let anchor_id = FederationCheckpointId::from_bytes([41; 32]);
    let barrier = FederationRecoveryBarrier::new(federation_id, 7, Some(anchor_id), 53)
        .expect("recovery barrier");
    let repeated = FederationRecoveryBarrier::new(federation_id, 7, Some(anchor_id), 53)
        .expect("same recovery barrier");

    assert_eq!(barrier.generation, repeated.generation);
    assert_eq!(
        barrier.generation.to_hex(),
        "6e7c905c157249097614bf915f18d0d7861c1de8da1de4db85ccc1489aa02887"
    );
    let encoded = serde_json::to_vec(&barrier).expect("serialize barrier");
    let decoded: FederationRecoveryBarrier =
        serde_json::from_slice(&encoded).expect("deserialize barrier");
    assert_eq!(decoded, barrier);
    decoded.ensure_supported_version().expect("valid barrier");

    let mut tampered = barrier.clone();
    tampered.requested_after_finalized_item_index = 54;
    assert!(matches!(
        tampered.ensure_supported_version(),
        Err(FederationError::CheckpointInvalid { reason })
            if reason.contains("generation does not match")
    ));
}

#[test]
fn recovery_barrier_is_bound_to_its_aleph_bft_federation() {
    let expected_federation = federation_id(32);
    let other_federation = federation_id(33);
    let item = FederationConsensusItem::recovery_barrier(recovery_barrier(other_federation, 0));

    assert!(matches!(
        FederationAlephBftUnitPayload::new(
            expected_federation,
            member_id(),
            0,
            0,
            vec![item],
        ),
        Err(FederationError::CheckpointInvalid { reason })
            if reason.contains("does not match its AlephBFT unit")
    ));
}

#[test]
fn consensus_runtime_allows_current_barrier_to_supersede_stale_pending_work() {
    let federation_id = federation_id(34);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id(), 0);

    assert!(runtime
        .submit_aleph_bft_recovery_barrier(recovery_barrier(federation_id, 0))
        .expect("submit first barrier"));
    assert!(!runtime
        .submit_aleph_bft_recovery_barrier(recovery_barrier(federation_id, 0))
        .expect("deduplicate covered barrier"));
    assert!(!runtime
        .submit_aleph_bft_recovery_barrier(recovery_barrier(federation_id, 1))
        .expect("reject future replacement barrier"));
    assert!(runtime.has_pending_aleph_bft_recovery_barrier());

    let advancing_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("advance-before-stale-barrier")],
    )
    .expect("advancing unit");
    runtime
        .finalize_aleph_bft_unit(advancing_unit)
        .expect("advance finalized cursor");

    assert!(runtime
        .submit_aleph_bft_recovery_barrier(recovery_barrier(federation_id, 1))
        .expect("submit current replacement barrier"));
    assert!(!runtime
        .submit_aleph_bft_recovery_barrier(recovery_barrier(federation_id, 1))
        .expect("deduplicate replacement barrier"));

    let unit = runtime
        .next_aleph_bft_unit_payload()
        .expect("next unit")
        .expect("barrier unit");
    assert_eq!(unit.items.len(), 2);
    runtime
        .finalize_aleph_bft_unit(unit)
        .expect("finalize barrier unit");
    assert!(!runtime.has_pending_aleph_bft_recovery_barrier());
}

#[test]
fn future_recovery_cursor_is_rejected_without_rejecting_its_unit() {
    let federation_id = federation_id(35);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id(), 0);
    let unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id(),
        0,
        0,
        vec![
            FederationConsensusItem::recovery_barrier(recovery_barrier(federation_id, 1)),
            module_fact_item("after-invalid-barrier"),
        ],
    )
    .expect("unit with invalid barrier cursor");

    runtime
        .finalize_unit(unit)
        .expect("unit remains finalizable");
    let finalized = runtime.finalized_items();
    assert_eq!(finalized.len(), 2);
    assert_eq!(
        finalized[0].status,
        FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier
    );
    assert_eq!(
        finalized[1].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
}

#[test]
fn consensus_item_queue_rejects_pending_conflicting_mint_quote() {
    let federation_id = federation_id(13);
    let first = mint_quote_envelope_with_amount(federation_id, "quote-conflict", 1);
    let conflict = mint_quote_envelope_with_amount(federation_id, "quote-conflict", 2);
    assert_ne!(
        first.operation_id().expect("first operation id"),
        conflict.operation_id().expect("conflict operation id")
    );
    let mut queue = FederationConsensusItemQueue::new();

    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(first))
            .expect("submit first quote"),
        FederationConsensusItemAdmission::Accepted
    );
    let err = queue
        .submit(FederationConsensusItem::operation(conflict.clone()))
        .expect_err("conflicting quote should be rejected while first is pending");

    assert!(matches!(err, FederationError::ConflictingOperation { .. }));
    assert_eq!(queue.len(), 1);

    let batch = queue.drain_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(conflict))
            .expect("conflict key is released after drain"),
        FederationConsensusItemAdmission::Accepted
    );
}

#[test]
fn consensus_item_queue_rejects_pending_conflicting_melt_quote() {
    let federation_id = federation_id(13);
    let first = melt_quote_envelope(federation_id, "melt-quote-conflict", CurrencyUnit::Sat);
    let conflict = melt_quote_envelope(federation_id, "melt-quote-conflict", CurrencyUnit::Msat);
    assert_ne!(
        first.operation_id().expect("first operation id"),
        conflict.operation_id().expect("conflict operation id")
    );
    let mut queue = FederationConsensusItemQueue::new();

    assert_eq!(
        queue
            .submit(FederationConsensusItem::operation(first))
            .expect("submit first melt quote"),
        FederationConsensusItemAdmission::Accepted
    );
    let err = queue
        .submit(FederationConsensusItem::operation(conflict))
        .expect_err("conflicting melt quote should be rejected while first is pending");

    assert!(matches!(err, FederationError::ConflictingOperation { .. }));
    assert_eq!(queue.len(), 1);
}

#[test]
fn consensus_item_queue_rejects_oversized_items() {
    let mut queue = FederationConsensusItemQueue::with_max_item_bytes(128);
    let item = FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
        "large",
        json!({"body": "x".repeat(256)}),
    ));

    assert!(matches!(
        queue.submit(item),
        Err(FederationError::ConsensusItemTooLarge { .. })
    ));
    assert!(queue.is_empty());
}

#[test]
fn consensus_item_queue_rejects_future_module_fact_versions() {
    let mut queue = FederationConsensusItemQueue::new();
    let mut fact = FederationConsensusModuleFact::new("mint", "fact", json!({"ok": true}));
    fact.version = FEDERATION_CONSENSUS_ITEM_VERSION + 1;
    let item = FederationConsensusItem::ModuleFact(fact);

    assert!(matches!(
        item.canonical_bytes(),
        Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found,
        }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
    ));
    assert!(matches!(
        queue.submit(item),
        Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found,
        }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
    ));
    assert!(queue.is_empty());
}

#[test]
fn consensus_item_queue_rejects_future_default_item_versions() {
    let mut queue = FederationConsensusItemQueue::new();
    let mut item = FederationConsensusDefaultItem::new("future", json!({"ok": true}));
    item.version = FEDERATION_CONSENSUS_ITEM_VERSION + 1;

    assert!(matches!(
        queue.submit(FederationConsensusItem::Default(item)),
        Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found,
        }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
    ));
    assert!(queue.is_empty());
}

#[test]
fn consensus_item_canonical_bytes_sort_json_payload_keys() {
    let first = FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
        "mint",
        "fact",
        json!({"b": 2, "a": {"d": 4, "c": 3}}),
    ));
    let second = FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
        "mint",
        "fact",
        json!({"a": {"c": 3, "d": 4}, "b": 2}),
    ));

    assert_eq!(
        first.canonical_bytes().expect("first canonical bytes"),
        second.canonical_bytes().expect("second canonical bytes")
    );
}

#[test]
fn signed_submit_request_maps_to_consensus_item_without_changing_operation_bytes() {
    let (config, private_members) = consensus_config_three();
    let receiver = private_members[1].member_id;
    let envelope = mint_envelope(
        config.federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let canonical_operation = envelope.canonical_bytes().expect("canonical operation");
    let request = FederationSignedRequest::new_signed(
        config.federation_id,
        receiver,
        7,
        FederationRestMessageKind::SubmitOperation,
        FederationSubmitOperationRequest::new(envelope),
        &private_members[0],
    )
    .expect("signed request");

    let admission = FederationVerifiedOperationAdmission::from_signed_submit_request(
        &request, &config, receiver,
    )
    .expect("verified admission");

    assert_eq!(admission.sender, private_members[0].member_id);
    assert_eq!(admission.sequence, 7);
    assert_eq!(admission.operation_id, operation_id);
    assert_eq!(
        admission.item.operation_id().expect("item operation id"),
        Some(operation_id)
    );

    let FederationConsensusItem::Operation(admitted_envelope) = &admission.item else {
        panic!("operation item");
    };
    assert_eq!(
        admitted_envelope
            .canonical_bytes()
            .expect("admitted canonical operation"),
        canonical_operation
    );
}

#[test]
fn signed_submit_request_admission_rejects_wrong_rest_kind() {
    let (config, private_members) = consensus_config_three();
    let receiver = private_members[1].member_id;
    let request = FederationSignedRequest::new_signed(
        config.federation_id,
        receiver,
        7,
        FederationRestMessageKind::Status,
        FederationSubmitOperationRequest::new(mint_envelope(
            config.federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        )),
        &private_members[0],
    )
    .expect("signed request");

    assert!(matches!(
        FederationVerifiedOperationAdmission::from_signed_submit_request(
            &request, &config, receiver
        ),
        Err(FederationError::RestMessageKindMismatch {
            expected: FederationRestMessageKind::SubmitOperation,
            found: FederationRestMessageKind::Status,
        })
    ));
}

#[test]
fn signed_submit_request_admission_rejects_unknown_member() {
    let (config, private_members) = consensus_config_three();
    let receiver = private_members[1].member_id;
    let unknown_member = private_member(4, identity_secret(4));
    let request = FederationSignedRequest::new_signed(
        config.federation_id,
        receiver,
        7,
        FederationRestMessageKind::SubmitOperation,
        FederationSubmitOperationRequest::new(mint_envelope(
            config.federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        )),
        &unknown_member,
    )
    .expect("signed request");

    assert!(matches!(
        FederationVerifiedOperationAdmission::from_signed_submit_request(&request, &config, receiver),
        Err(FederationError::UnknownMember { member_id })
            if member_id == unknown_member.member_id
    ));
}

#[test]
fn aleph_bft_unit_payload_commits_to_ordered_items_and_operation_ids() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let operation_item = FederationConsensusItem::operation(envelope);
    let fact_item = FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
        "mint",
        "fact",
        json!({"paid": true}),
    ));

    let payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        7,
        11,
        vec![operation_item.clone(), fact_item.clone()],
    )
    .expect("payload");
    let same_payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        7,
        11,
        vec![operation_item.clone(), fact_item.clone()],
    )
    .expect("same payload");
    let reversed_payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        7,
        11,
        vec![fact_item, operation_item],
    )
    .expect("reversed payload");

    assert_eq!(payload.version, FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION);
    assert_eq!(payload.federation_id, federation_id);
    assert_eq!(payload.submitting_member, member_id_value(2));
    assert_eq!(payload.session, 7);
    assert_eq!(payload.unit_index, 11);
    assert_eq!(payload.operation_ids, vec![operation_id]);
    let payload_json = serde_json::to_value(&payload).expect("payload json");
    assert!(payload_json.get("protocol_versions").is_none());
    assert!(payload_json.get("consensus_item_version").is_none());
    for field in ["protocol_versions", "consensus_item_version"] {
        let mut obsolete = payload_json.clone();
        obsolete[field] = json!({ "operation": 1 });
        serde_json::from_value::<FederationAlephBftUnitPayload>(obsolete)
            .expect_err("removed Aleph unit protocol metadata must fail closed");
    }
    assert_eq!(payload.body_hash, same_payload.body_hash);
    assert_eq!(
        payload.canonical_bytes().expect("payload canonical bytes"),
        same_payload
            .canonical_bytes()
            .expect("same payload canonical bytes")
    );
    assert_ne!(payload.body_hash, reversed_payload.body_hash);
    assert_eq!(reversed_payload.operation_ids, vec![operation_id]);
}

#[test]
fn aleph_bft_unit_payload_rejects_item_version_at_item_boundary() {
    let mut item = FederationConsensusModuleFact::new("mint", "future", json!({"ok": true}));
    item.version = FEDERATION_CONSENSUS_ITEM_VERSION + 1;

    assert!(matches!(
        FederationAlephBftUnitPayload::new(
            federation_id(13),
            member_id_value(2),
            7,
            0,
            vec![FederationConsensusItem::ModuleFact(item)],
        ),
        Err(FederationError::ConsensusItemVersionMismatch {
            expected: FEDERATION_CONSENSUS_ITEM_VERSION,
            found,
        }) if found == FEDERATION_CONSENSUS_ITEM_VERSION + 1
    ));
}

#[test]
fn aleph_bft_unit_payload_rejects_tampered_body_hash_and_operation_ids() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let item = FederationConsensusItem::operation(envelope);
    let mut payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        7,
        0,
        vec![item.clone()],
    )
    .expect("payload");

    payload.body_hash = FederationAlephBftUnitBodyHash::from_bytes([9; 32]);
    assert!(matches!(
        payload.ensure_supported_version(),
        Err(FederationError::AlephBftUnitBodyHashMismatch { .. })
    ));

    let mut payload =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 7, 0, vec![item])
            .expect("payload");
    payload.operation_ids.clear();
    assert!(matches!(
        payload.ensure_supported_version(),
        Err(FederationError::AlephBftUnitOperationIdsMismatch)
    ));

    let mut payload =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 7, 0, Vec::new())
            .expect("empty payload");
    payload.version = FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION + 1;
    assert!(matches!(
        payload.ensure_supported_version(),
        Err(FederationError::ProtocolVersionMismatch {
            component: "aleph_bft_unit_payload",
            expected: FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION,
            found,
        }) if found == FEDERATION_ALEPH_BFT_UNIT_PAYLOAD_VERSION + 1
    ));
}

#[test]
fn aleph_bft_data_provider_drains_bounded_batches_without_empty_units() {
    let federation_id = federation_id(13);
    let mut provider = FederationAlephBftUnitDataProvider::with_limits(
        federation_id,
        member_id_value(2),
        3,
        FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
        2,
        FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
    )
    .expect("provider");

    for index in 0..3 {
        provider
            .submit_item(FederationConsensusItem::ModuleFact(
                FederationConsensusModuleFact::new("mint", "fact", json!({"index": index})),
            ))
            .expect("submit fact");
    }

    let first = provider
        .next_unit_payload()
        .expect("first unit")
        .expect("first unit present");
    assert_eq!(first.session, 3);
    assert_eq!(first.unit_index, 0);
    assert_eq!(first.items.len(), 2);
    assert_eq!(provider.queued_len(), 1);
    assert_eq!(provider.next_unit_index(), 1);

    let second = provider
        .next_unit_payload()
        .expect("second unit")
        .expect("second unit present");
    assert_eq!(second.unit_index, 1);
    assert_eq!(second.items.len(), 1);
    assert!(provider.is_empty());
    assert_eq!(provider.next_unit_index(), 2);

    assert!(provider
        .next_unit_payload()
        .expect("empty unit check")
        .is_none());
    assert_eq!(provider.next_unit_index(), 2);

    provider.begin_session(4);
    assert_eq!(provider.session(), 4);
    assert_eq!(provider.next_unit_index(), 0);
}

#[test]
fn aleph_bft_data_provider_empty_queue_repeated_polls_do_not_advance() {
    let federation_id = federation_id(13);
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 11);

    for _ in 0..8 {
        assert!(provider
            .next_unit_payload()
            .expect("empty next unit")
            .is_none());
        assert!(provider.is_empty());
        assert_eq!(provider.session(), 11);
        assert_eq!(provider.queued_len(), 0);
        assert_eq!(provider.next_unit_index(), 0);
    }

    provider
        .submit_item(FederationConsensusItem::Default(
            FederationConsensusDefaultItem::new("after-empty-polls", json!({"ready": true})),
        ))
        .expect("submit item after empty polls");
    let payload = provider
        .next_unit_payload()
        .expect("first non-empty unit")
        .expect("unit present");

    assert_eq!(payload.session, 11);
    assert_eq!(payload.unit_index, 0);
    assert_eq!(payload.items.len(), 1);
    assert_eq!(provider.next_unit_index(), 1);
}

#[test]
fn aleph_bft_data_provider_deduplicates_pending_operations() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 0);

    assert_eq!(
        provider
            .submit_operation(envelope.clone())
            .expect("submit operation"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        provider
            .submit_operation(envelope)
            .expect("submit duplicate operation"),
        FederationConsensusItemAdmission::DuplicateOperation
    );

    let payload = provider
        .next_unit_payload()
        .expect("unit")
        .expect("unit present");
    assert_eq!(payload.items.len(), 1);
    assert_eq!(payload.operation_ids, vec![operation_id]);
    assert!(provider.is_empty());
}

#[test]
fn aleph_bft_data_provider_rejects_pending_conflicting_quote_operations() {
    let federation_id = federation_id(13);
    let first = mint_quote_envelope_with_amount(federation_id, "quote-aleph-conflict", 1);
    let conflict = mint_quote_envelope_with_amount(federation_id, "quote-aleph-conflict", 2);
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 0);

    assert_eq!(
        provider
            .submit_operation(first)
            .expect("submit first operation"),
        FederationConsensusItemAdmission::Accepted
    );
    let err = provider
        .submit_operation(conflict)
        .expect_err("conflicting pending operation should be rejected");

    assert!(matches!(err, FederationError::ConflictingOperation { .. }));
    assert_eq!(provider.queued_len(), 1);
}

#[test]
fn aleph_bft_data_provider_enforces_body_byte_limit_without_spinning_or_dropping() {
    let federation_id = federation_id(13);
    let first = FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
        "small",
        json!({"body": "a"}),
    ));
    let second = FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
        "small",
        json!({"body": "b"}),
    ));
    let first_size = first.canonical_size().expect("first size");
    let mut provider = FederationAlephBftUnitDataProvider::with_limits(
        federation_id,
        member_id(),
        0,
        FEDERATION_MAX_CONSENSUS_ITEM_BYTES,
        10,
        first_size,
    )
    .expect("provider");

    provider.submit_item(first).expect("submit first");
    provider.submit_item(second).expect("submit second");

    let payload = provider
        .next_unit_payload()
        .expect("unit")
        .expect("unit present");
    assert_eq!(payload.items.len(), 1);
    assert_eq!(provider.queued_len(), 1);

    let oversized = FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
        "too-large-for-unit",
        json!({"body": "x".repeat(64)}),
    ));
    let oversized_size = oversized.canonical_size().expect("oversized size");
    let mut provider = FederationAlephBftUnitDataProvider::with_limits(
        federation_id,
        member_id(),
        0,
        oversized_size + 1,
        10,
        oversized_size - 1,
    )
    .expect("provider");
    provider
        .submit_item(oversized)
        .expect("submit oversized-for-unit item");

    assert!(matches!(
        provider.next_unit_payload(),
        Err(FederationError::ConsensusItemTooLarge { .. })
    ));
    assert_eq!(provider.queued_len(), 1);
}

#[test]
fn aleph_bft_data_provider_rejects_zero_limits() {
    let err = FederationAlephBftUnitDataProvider::with_limits(
        federation_id(13),
        member_id(),
        0,
        0,
        FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
        FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
    )
    .expect_err("zero item size limit");

    assert!(matches!(
        err,
        FederationError::InvalidConsensusLimit {
            field: "max_item_bytes",
            value: 0,
        }
    ));
}

#[test]
fn aleph_bft_finalization_handler_emits_ordered_items_with_unit_metadata() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        5,
        vec![
            FederationConsensusItem::operation(envelope),
            FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
                "mint",
                "payment_observation",
                json!({"quote": "quote-1", "paid": true}),
            )),
        ],
    )
    .expect("payload");
    let body_hash = payload.body_hash;
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);

    let outcome = handler
        .finalize_next_unit(payload)
        .expect("finalized outcome");

    assert_eq!(outcome.session, 0);
    assert_eq!(outcome.unit_finalization_index, 0);
    assert_eq!(outcome.unit_submitting_member, member_id_value(2));
    assert_eq!(outcome.unit_index, 5);
    assert_eq!(outcome.unit_body_hash, body_hash);
    assert_eq!(outcome.items.len(), 2);
    assert!(outcome.duplicate_operation_ids().is_empty());

    assert_eq!(outcome.items[0].accepted_item_index, 0);
    assert_eq!(outcome.items[0].item_index_in_unit, 0);
    assert_eq!(outcome.items[0].operation_id, Some(operation_id));
    assert_eq!(
        outcome.items[0].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
    assert_eq!(outcome.items[0].unit_body_hash, body_hash);

    assert_eq!(outcome.items[1].accepted_item_index, 1);
    assert_eq!(outcome.items[1].item_index_in_unit, 1);
    assert_eq!(outcome.items[1].operation_id, None);
    assert_eq!(
        outcome.items[1].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
    assert_eq!(handler.next_unit_finalization_index(), 1);
    assert_eq!(handler.next_item_index(), 2);
    assert_eq!(
        handler.next_unit_index_by_member().get(&member_id_value(2)),
        Some(&6)
    );
}

#[test]
fn aleph_bft_finalization_handler_rejects_balance_overflow_operation_per_item() {
    let federation_id = federation_id(13);
    let overflow_output = |hex: &str| {
        BlindedMessage::new(
            Amount::from(u64::MAX),
            keyset_id(),
            PublicKey::from_hex(hex).expect("public key"),
        )
    };
    let invalid = FederationOperationEnvelope::new(
        federation_id,
        super::super::FederationOperation::Mint(super::super::FederationMintOperation {
            request: super::super::FederationMintRequest::Single(MintRequest {
                quote: "overflow-quote".to_string(),
                outputs: vec![
                    overflow_output(
                        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                    ),
                    overflow_output(
                        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
                    ),
                ],
                signature: None,
            }),
        }),
    );
    let invalid_operation_id = invalid.operation_id().expect("invalid operation id");
    let same_unit_valid = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let same_unit_valid_operation_id = same_unit_valid.operation_id().expect("operation id");
    let payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![
            FederationConsensusItem::operation(invalid),
            FederationConsensusItem::operation(same_unit_valid),
        ],
    )
    .expect("payload");
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);

    let outcome = handler
        .finalize_next_unit(payload)
        .expect("unit with one invalid item still finalizes");

    assert_eq!(outcome.items.len(), 2);
    assert_eq!(outcome.items[0].operation_id, Some(invalid_operation_id));
    assert_eq!(
        outcome.items[0].status,
        FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation
    );
    assert_eq!(
        outcome.items[1].operation_id,
        Some(same_unit_valid_operation_id)
    );
    assert_eq!(
        outcome.items[1].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );

    let next_unit_valid = mint_envelope(
        federation_id,
        "quote-2",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let next_unit_valid_operation_id = next_unit_valid.operation_id().expect("operation id");
    let next_payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        1,
        vec![FederationConsensusItem::operation(next_unit_valid)],
    )
    .expect("next payload");

    let next_outcome = handler
        .finalize_next_unit(next_payload)
        .expect("next unit finalizes after the rejected item");

    assert_eq!(
        next_outcome.items[0].operation_id,
        Some(next_unit_valid_operation_id)
    );
    assert_eq!(
        next_outcome.items[0].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
    assert_eq!(next_outcome.items[0].accepted_item_index, 2);
}

#[test]
fn aleph_bft_finalization_handler_rejects_wrong_federation_and_session() {
    let expected_federation_id = federation_id(13);
    let wrong_federation_payload =
        FederationAlephBftUnitPayload::new(federation_id(14), member_id(), 0, 0, Vec::new())
            .expect("wrong federation payload");
    let wrong_session_payload =
        FederationAlephBftUnitPayload::new(expected_federation_id, member_id(), 1, 0, Vec::new())
            .expect("wrong session payload");
    let mut handler = FederationAlephBftFinalizationHandler::new(expected_federation_id);

    assert!(matches!(
        handler.finalize_next_unit(wrong_federation_payload),
        Err(FederationError::FederationIdMismatch {
            expected,
            found,
        }) if expected == expected_federation_id && found == federation_id(14)
    ));
    assert!(matches!(
        handler.finalize_next_unit(wrong_session_payload),
        Err(FederationError::AlephBftUnitSessionMismatch {
            expected: 0,
            found: 1,
        })
    ));
}

#[test]
fn aleph_bft_finalization_handler_flags_duplicate_operations_in_order() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let first_payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first payload");
    let duplicate_payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(3),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("duplicate payload");
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);

    let first = handler
        .finalize_next_unit(first_payload)
        .expect("first finalized unit");
    let duplicate = handler
        .finalize_next_unit(duplicate_payload)
        .expect("duplicate finalized unit");

    assert_eq!(first.items[0].accepted_item_index, 0);
    assert_eq!(
        first.items[0].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
    assert_eq!(duplicate.items[0].accepted_item_index, 1);
    assert_eq!(duplicate.items[0].operation_id, Some(operation_id));
    assert_eq!(
        duplicate.items[0].status,
        FederationAlephBftFinalizedItemStatus::DuplicateOperation
    );
    assert_eq!(duplicate.duplicate_operation_ids(), vec![operation_id]);
    assert_eq!(handler.next_unit_finalization_index(), 2);
    assert_eq!(handler.next_item_index(), 2);
}

#[test]
fn aleph_bft_finalization_handler_rejects_duplicate_member_unit_replay() {
    let federation_id = federation_id(13);
    let payload = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("duplicate-unit-replay")],
    )
    .expect("payload");
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);

    handler
        .finalize_next_unit(payload.clone())
        .expect("first finalized unit");

    assert!(matches!(
        handler.finalize_next_unit(payload),
        Err(FederationError::DuplicateAlephBftUnit {
            session: 0,
            member_id,
            unit_index: 0,
        }) if member_id == member_id_value(2)
    ));
    assert_eq!(handler.next_unit_finalization_index(), 1);
}

#[test]
fn aleph_bft_finalization_handler_rejects_empty_unit_before_state_change() {
    let federation_id = federation_id(13);
    let empty_unit =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 0, 0, Vec::new())
            .expect("empty unit");
    let non_empty_replay = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("empty-unit-replay")],
    )
    .expect("non-empty replay");
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);

    assert!(matches!(
        handler.finalize_next_unit(empty_unit),
        Err(FederationError::EmptyAlephBftUnit)
    ));
    assert_eq!(handler.next_unit_finalization_index(), 0);
    assert_eq!(handler.next_item_index(), 0);

    let outcome = handler
        .finalize_next_unit(non_empty_replay)
        .expect("same unit can finalize after empty rejection");
    assert_eq!(outcome.unit_finalization_index, 0);
    assert_eq!(outcome.items.len(), 1);
}

#[test]
fn aleph_bft_finalization_handler_rejects_unknown_submitter_before_state_change() {
    let federation_id = federation_id(13);
    let mut handler = FederationAlephBftFinalizationHandler::with_allowed_members(
        federation_id,
        0,
        0,
        FEDERATION_MAX_ALEPH_BFT_UNIT_PAYLOAD_BYTES,
        [member_id_value(1), member_id_value(2), member_id_value(3)],
    )
    .expect("handler");
    let unknown_unit =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(4), 0, 0, Vec::new())
            .expect("unknown member unit");

    assert!(matches!(
        handler.finalize_next_unit(unknown_unit),
        Err(FederationError::UnknownMember { member_id })
            if member_id == member_id_value(4)
    ));
    assert_eq!(handler.next_unit_finalization_index(), 0);
    assert_eq!(handler.next_item_index(), 0);

    let known_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("known-member")],
    )
    .expect("known member unit");
    handler
        .finalize_next_unit(known_unit)
        .expect("known member unit finalized");
    assert_eq!(handler.next_unit_finalization_index(), 1);
}

#[test]
fn aleph_bft_finalization_handler_advances_to_later_sessions() {
    let federation_id = federation_id(13);
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);
    let first = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("session-0")],
    )
    .expect("first payload");
    handler.finalize_next_unit(first).expect("first finalized");

    handler.begin_session(1).expect("begin next session");
    assert_eq!(handler.session(), 1);
    assert_eq!(handler.next_unit_finalization_index(), 0);
    assert_eq!(handler.next_item_index(), 1);

    let next = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        1,
        0,
        vec![module_fact_item("session-1")],
    )
    .expect("next payload");
    handler.finalize_next_unit(next).expect("next finalized");
    assert!(matches!(
        handler.begin_session(1),
        Err(FederationError::AlephBftUnitSessionMismatch {
            expected: 2,
            found: 1,
        })
    ));
}

#[test]
fn consensus_runtime_builds_units_and_applies_finalized_operations() {
    let federation_id = federation_id(13);
    let first = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let first_id = first.operation_id().expect("first operation id");
    let second = mint_envelope(
        federation_id,
        "quote-2",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let second_id = second.operation_id().expect("second operation id");
    let mut adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);

    assert_eq!(
        adapter.submit_operation(first).expect("submit first"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        adapter.submit_operation(second).expect("submit second"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(adapter.queued_len(), 2);

    let unit = adapter
        .next_unit_payload()
        .expect("next unit")
        .expect("unit present");
    assert_eq!(unit.submitting_member, member_id_value(1));
    assert_eq!(unit.session, 0);
    assert_eq!(unit.unit_index, 0);
    assert_eq!(unit.operation_ids, vec![first_id, second_id]);
    assert_eq!(adapter.queued_len(), 0);
    assert_eq!(adapter.next_unit_index(), 1);

    let responses = adapter.finalize_unit(unit).expect("finalize unit");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].operation_id, first_id);
    assert_eq!(responses[0].outcome.entry().index, 0);
    assert_eq!(responses[1].operation_id, second_id);
    assert_eq!(responses[1].outcome.entry().index, 1);
    assert_eq!(adapter.journal().len(), 2);
    let first_entry = &adapter.journal().entries()[0];
    let first_evidence = first_entry.consensus_evidence.expect("first evidence");
    assert_eq!(first_evidence.session, 0);
    assert_eq!(first_evidence.item_index, 0);
    assert_eq!(first_evidence.unit_finalization_index, 0);
    assert_eq!(first_evidence.unit_submitting_member, member_id_value(1));
    assert_eq!(first_evidence.unit_index, 0);
    assert_eq!(adapter.finalized_items().len(), 2);
    assert_eq!(
        adapter
            .finalized_items()
            .iter()
            .map(|item| item.operation_id)
            .collect::<Vec<_>>(),
        vec![Some(first_id), Some(second_id)]
    );
}

#[test]
fn consensus_runtime_marks_duplicate_finalized_operations_idempotently() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let first_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first unit");
    let duplicate_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(3),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("duplicate unit");
    let mut adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);

    let first_response = adapter
        .finalize_unit(first_unit)
        .expect("first finalization");
    let duplicate_response = adapter
        .finalize_unit(duplicate_unit)
        .expect("duplicate finalization");

    assert_eq!(first_response.len(), 1);
    assert_eq!(first_response[0].outcome.entry().index, 0);
    assert_eq!(duplicate_response.len(), 1);
    assert_eq!(duplicate_response[0].operation_id, operation_id);
    assert_eq!(duplicate_response[0].outcome.entry().index, 0);
    assert_eq!(adapter.journal().len(), 1);
    assert_eq!(
        adapter.finalized_items()[1].status,
        FederationAlephBftFinalizedItemStatus::DuplicateOperation
    );
}

#[test]
fn consensus_runtime_finalizes_four_member_order_with_faulty_duplicates() {
    let federation_id = federation_id(13);
    let members = member_roster(4);
    let first = mint_quote_envelope_with_amount(federation_id, "quote-four-a", 1);
    let second = mint_quote_envelope_with_amount(federation_id, "quote-four-b", 1);
    let duplicate_second = second.clone();
    let conflicting_first = mint_quote_envelope_with_amount(federation_id, "quote-four-a", 2);
    let first_id = first.operation_id().expect("first operation id");
    let second_id = second.operation_id().expect("second operation id");
    let conflict_id = conflicting_first
        .operation_id()
        .expect("conflict operation id");
    let units = [
        aleph_operation_unit(
            federation_id,
            members[0],
            0,
            0,
            vec![first.clone(), second.clone()],
        ),
        aleph_operation_unit(federation_id, members[1], 0, 0, vec![duplicate_second]),
        aleph_operation_unit(federation_id, members[2], 0, 0, vec![conflicting_first]),
    ];
    let mut adapters = aleph_adapters_for_roster(federation_id, &members);

    for adapter in &mut adapters {
        let first_responses = adapter
            .finalize_unit(units[0].clone())
            .expect("first four-member unit finalizes");
        assert_eq!(first_responses.len(), 2);
        assert_eq!(first_responses[0].operation_id, first_id);
        assert_eq!(first_responses[1].operation_id, second_id);

        let duplicate_responses = adapter
            .finalize_unit(units[1].clone())
            .expect("duplicate four-member unit finalizes idempotently");
        assert_eq!(duplicate_responses.len(), 1);
        assert_eq!(duplicate_responses[0].operation_id, second_id);
        assert_eq!(duplicate_responses[0].outcome.entry().index, 1);

        let conflict_responses = adapter
            .finalize_unit(units[2].clone())
            .expect("conflicting four-member unit finalizes deterministically");
        assert_eq!(conflict_responses.len(), 1);
        assert_eq!(conflict_responses[0].operation_id, conflict_id);
        assert_eq!(
            conflict_responses[0].outcome.entry().status(),
            FederationOperationStatus::Rejected
        );
    }

    assert_aleph_adapters_converged(&adapters);
    let first_adapter = &adapters[0];
    assert_eq!(first_adapter.journal().len(), 3);
    assert_eq!(
        first_adapter
            .journal()
            .entries()
            .iter()
            .map(FederationJournalEntry::status)
            .collect::<Vec<_>>(),
        vec![
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Rejected,
        ]
    );
    assert_eq!(
        first_adapter
            .finalized_items()
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            FederationAlephBftFinalizedItemStatus::Accepted,
            FederationAlephBftFinalizedItemStatus::Accepted,
            FederationAlephBftFinalizedItemStatus::DuplicateOperation,
            FederationAlephBftFinalizedItemStatus::Accepted,
        ]
    );
    let status = first_adapter.status();
    assert_eq!(status.allowed_member_count, Some(4));
    assert_eq!(status.finalized_item_count, 4);
    assert_eq!(status.accepted_operation_count, 3);
    assert_eq!(status.latest_accepted_operation_index, Some(2));
}

#[test]
fn consensus_runtime_finalizes_five_member_sessions_with_silent_member() {
    let federation_id = federation_id(14);
    let members = member_roster(5);
    let session_zero_first = mint_quote_envelope_with_amount(federation_id, "quote-five-a", 1);
    let session_zero_second = mint_quote_envelope_with_amount(federation_id, "quote-five-b", 1);
    let session_one_first = mint_quote_envelope_with_amount(federation_id, "quote-five-c", 1);
    let session_one_second = mint_quote_envelope_with_amount(federation_id, "quote-five-d", 1);
    let session_one_duplicate = session_zero_first.clone();
    let units = vec![
        aleph_operation_unit(
            federation_id,
            members[0],
            0,
            0,
            vec![session_zero_first.clone()],
        ),
        aleph_operation_unit(
            federation_id,
            members[1],
            0,
            0,
            vec![session_zero_second.clone()],
        ),
    ];
    let session_one_units = vec![
        aleph_operation_unit(
            federation_id,
            members[3],
            1,
            0,
            vec![session_one_first.clone(), session_one_duplicate],
        ),
        aleph_operation_unit(
            federation_id,
            members[4],
            1,
            0,
            vec![session_one_second.clone()],
        ),
    ];
    let mut adapters = aleph_adapters_for_roster(federation_id, &members);

    for adapter in &mut adapters {
        for unit in &units {
            adapter
                .finalize_unit(unit.clone())
                .expect("five-member session zero unit finalizes");
        }
        adapter.begin_session(1).expect("begin session one");
        for unit in &session_one_units {
            adapter
                .finalize_unit(unit.clone())
                .expect("five-member session one unit finalizes");
        }
    }

    assert_aleph_adapters_converged(&adapters);
    let first_adapter = &adapters[0];
    assert_eq!(first_adapter.journal().len(), 4);
    assert_eq!(
        first_adapter
            .journal()
            .entries()
            .iter()
            .map(FederationJournalEntry::status)
            .collect::<Vec<_>>(),
        vec![
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Accepted,
            FederationOperationStatus::Accepted,
        ]
    );
    assert_eq!(
        first_adapter
            .finalized_items()
            .iter()
            .map(|item| (item.session, item.unit_submitting_member, item.status))
            .collect::<Vec<_>>(),
        vec![
            (
                0,
                members[0],
                FederationAlephBftFinalizedItemStatus::Accepted,
            ),
            (
                0,
                members[1],
                FederationAlephBftFinalizedItemStatus::Accepted,
            ),
            (
                1,
                members[3],
                FederationAlephBftFinalizedItemStatus::Accepted,
            ),
            (
                1,
                members[3],
                FederationAlephBftFinalizedItemStatus::DuplicateOperation,
            ),
            (
                1,
                members[4],
                FederationAlephBftFinalizedItemStatus::Accepted,
            ),
        ]
    );
    assert!(
        first_adapter
            .finalized_items()
            .iter()
            .all(|item| item.unit_submitting_member != members[2]),
        "member three is intentionally silent in this five-member finalization run"
    );
    let status = first_adapter.status();
    assert_eq!(status.allowed_member_count, Some(5));
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_finalization_index, 2);
    assert_eq!(status.next_item_index, 5);
    assert_eq!(status.finalized_item_count, 5);
    assert_eq!(status.accepted_operation_count, 4);
    assert_eq!(status.latest_accepted_operation_index, Some(3));
}

#[test]
fn consensus_runtime_recovers_from_finalized_items() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    let first_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first unit");
    let duplicate_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(3),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("duplicate unit");
    let later_duplicate_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        1,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("later duplicate unit");
    let mut adapter = FederationConsensusRuntime::for_members(
        federation_id,
        member_id_value(1),
        0,
        [member_id_value(1), member_id_value(2), member_id_value(3)],
    )
    .expect("adapter");

    adapter.finalize_unit(first_unit).expect("first finalized");
    adapter
        .finalize_unit(duplicate_unit.clone())
        .expect("duplicate finalized");
    let finalized_items = adapter.finalized_items().to_vec();
    let recovered_runtime = recover_with_roster_for_test(
        federation_id,
        member_id_value(1),
        0,
        [member_id_value(1), member_id_value(2), member_id_value(3)],
        2,
        finalized_items.clone(),
    )
    .expect("recovered runtime");
    assert_eq!(
        recovered_runtime
            .aleph_bft_finalized_items()
            .expect("runtime finalized items")
            .len(),
        2
    );
    assert_eq!(
        recovered_runtime
            .aleph_bft_status()
            .expect("runtime AlephBFT status")
            .next_unit_finalization_index,
        2
    );

    let mut recovered = recover_for_members_for_test(
        federation_id,
        member_id_value(1),
        0,
        [member_id_value(1), member_id_value(2), member_id_value(3)],
        finalized_items,
    )
    .expect("recovered adapter");

    assert_eq!(recovered.journal().len(), 1);
    assert_eq!(recovered.journal().entries()[0].operation_id, operation_id);
    assert_eq!(recovered.finalized_items().len(), 2);
    assert_eq!(recovered.next_item_index(), 2);
    assert_eq!(recovered.status().next_unit_finalization_index, 2);
    assert_eq!(
        recovered.next_unit_index_by_member(),
        &BTreeMap::from([
            (member_id_value(1), 0),
            (member_id_value(2), 1),
            (member_id_value(3), 1),
        ])
    );
    assert!(matches!(
        recovered.finalize_unit(duplicate_unit),
        Err(FederationError::DuplicateAlephBftUnit {
            session: 0,
            member_id,
            unit_index: 0,
        }) if member_id == member_id_value(3)
    ));

    let duplicate_response = recovered
        .finalize_unit(later_duplicate_unit)
        .expect("later duplicate finalized");
    assert_eq!(duplicate_response.len(), 1);
    assert_eq!(duplicate_response[0].operation_id, operation_id);
    assert_eq!(duplicate_response[0].outcome.entry().index, 0);
    assert_eq!(recovered.journal().len(), 1);
    assert_eq!(
        recovered.finalized_items()[2].status,
        FederationAlephBftFinalizedItemStatus::DuplicateOperation
    );
}

#[test]
fn consensus_runtime_starts_at_a_trusted_checkpoint_without_prefix_rows() {
    let (config, _) = consensus_config_three();
    let boundary = super::super::FederationCheckpointBoundary {
        journal_next_index: 1_000,
        journal_order_digest: super::super::FederationJournalDigest::from_bytes([31; 32]),
        finalized_next_item_index: 1_200,
        finalized_item_digest: super::super::FederationFinalizedItemDigest::from_bytes([32; 32]),
        application_digest: super::super::FederationApplicationDigest::from_bytes([33; 32]),
        aleph_bft_session: 7,
        next_unit_finalization_index: 9,
        next_unit_index_by_member: BTreeMap::from([
            (member_id_value(1), 4),
            (member_id_value(2), 5),
            (member_id_value(3), 6),
        ]),
    };
    let checkpoint = boundary
        .recovery_checkpoint(
            &config,
            None,
            super::super::FederationMaterializedStateDigest::from_bytes([34; 32]),
        )
        .expect("recovery checkpoint");
    let mut runtime = FederationConsensusRuntime::checkpoint_base_for_test(
        &config,
        member_id_value(1),
        &checkpoint,
    )
    .expect("checkpoint runtime");

    let status = runtime.status();
    assert_eq!(status.accepted_operation_count, 1_000);
    assert_eq!(status.latest_accepted_operation_index, Some(999));
    assert_eq!(status.finalized_item_count, 1_200);
    assert_eq!(status.next_item_index, 1_200);
    assert_eq!(status.next_unit_finalization_index, 9);
    assert_eq!(runtime.journal().len(), 0);
    assert_eq!(runtime.finalized_items().len(), 0);
    assert_eq!(runtime.next_unit_index(), 4);
    assert_eq!(runtime.peer_status().operation_count, 1_000);
    assert_eq!(runtime.peer_status().latest_index, Some(999));

    let replay = FederationAlephBftUnitPayload::new(
        config.federation_id,
        member_id_value(2),
        7,
        4,
        vec![module_fact_item("replay")],
    )
    .expect("pre-checkpoint replay");
    assert!(matches!(
        runtime.finalize_unit(replay),
        Err(FederationError::DuplicateAlephBftUnit {
            session: 7,
            member_id,
            unit_index: 4,
        }) if member_id == member_id_value(2)
    ));

    let suffix = FederationAlephBftUnitPayload::new(
        config.federation_id,
        member_id_value(2),
        7,
        5,
        vec![module_fact_item("suffix")],
    )
    .expect("post-checkpoint unit");
    assert!(runtime
        .finalize_unit(suffix)
        .expect("finalize suffix")
        .is_empty());
    assert_eq!(runtime.finalized_items()[0].accepted_item_index, 1_200);
    assert_eq!(runtime.status().finalized_item_count, 1_201);
    assert_eq!(runtime.status().next_unit_finalization_index, 10);
    let suffix_canonical_bytes = u64::try_from(
        runtime.finalized_items()[0]
            .canonical_size()
            .expect("suffix canonical size"),
    )
    .expect("suffix canonical size fits u64");
    assert_eq!(
        runtime
            .retained_finalized_canonical_bytes_for_range(1_200, 1_201)
            .expect("suffix canonical byte range"),
        suffix_canonical_bytes
    );
    assert_eq!(
        runtime
            .retained_finalized_canonical_bytes_for_range(1_201, 1_201)
            .expect("empty canonical byte range"),
        0
    );
    assert!(runtime
        .retained_finalized_canonical_bytes_for_range(1_199, 1_201)
        .is_err());

    let recovered = FederationConsensusRuntime::recover_checkpoint_suffix_for_test(
        &config,
        member_id_value(1),
        7,
        &checkpoint,
        Vec::new(),
        runtime.finalized_items().to_vec(),
    )
    .expect("recover checkpoint suffix");
    assert_eq!(recovered.status().accepted_operation_count, 1_000);
    assert_eq!(recovered.status().finalized_item_count, 1_201);
    assert_eq!(recovered.status().next_unit_finalization_index, 10);
    assert_eq!(
        recovered.next_unit_index_by_member()[&member_id_value(2)],
        6
    );
    assert_eq!(recovered.finalized_items().len(), 1);
    assert_eq!(
        recovered
            .retained_finalized_canonical_bytes_for_range(1_200, 1_201)
            .expect("recovered suffix canonical byte range"),
        suffix_canonical_bytes
    );
}

#[test]
fn consensus_runtime_canonical_byte_ranges_remain_exact_at_large_history() {
    let federation_id = federation_id(44);
    let member_id = member_id_value(1);
    let mut runtime = FederationConsensusRuntime::new(federation_id, member_id, 0);
    let mut expected_canonical_bytes = 0_u64;
    let history_sizes = [20_u64, 200, 2_000, 10_000];

    for item_index in 0..10_000_u64 {
        let payload = FederationAlephBftUnitPayload::new(
            federation_id,
            member_id,
            0,
            item_index,
            vec![module_fact_item(&format!(
                "canonical-byte-item-{item_index}"
            ))],
        )
        .expect("large-history finalized payload");
        runtime
            .finalize_unit(payload)
            .expect("large-history finalized unit");
        expected_canonical_bytes = expected_canonical_bytes
            .checked_add(
                u64::try_from(
                    runtime
                        .finalized_items()
                        .last()
                        .expect("newly finalized item")
                        .canonical_size()
                        .expect("newly finalized item canonical size"),
                )
                .expect("canonical size fits u64"),
            )
            .expect("test canonical byte count does not overflow");

        let finalized_count = item_index + 1;
        if history_sizes.contains(&finalized_count) {
            assert_eq!(
                runtime
                    .retained_finalized_canonical_bytes_for_range(0, finalized_count)
                    .expect("large-history canonical byte range"),
                expected_canonical_bytes
            );
        }
    }

    let last_item_bytes = u64::try_from(
        runtime
            .finalized_items()
            .last()
            .expect("last large-history item")
            .canonical_size()
            .expect("last large-history item canonical size"),
    )
    .expect("last canonical size fits u64");
    for _ in 0..10_000 {
        assert_eq!(
            runtime
                .retained_finalized_canonical_bytes_for_range(9_999, 10_000)
                .expect("constant-work tail canonical byte range"),
            last_item_bytes
        );
    }
}

#[test]
fn consensus_runtime_recovery_preserves_finalized_conflict_rejection() {
    let federation_id = federation_id(14);
    let first = mint_quote_envelope_with_amount(federation_id, "quote-conflict", 1);
    let conflicting = mint_quote_envelope_with_amount(federation_id, "quote-conflict", 2);
    let unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![
            FederationConsensusItem::operation(first),
            FederationConsensusItem::operation(conflicting),
        ],
    )
    .expect("conflicting finalized unit");
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        FederationConsensusRuntime::for_members(federation_id, member_id_value(1), 0, members)
            .expect("adapter");
    adapter.finalize_unit(unit).expect("finalize conflicts");
    assert_eq!(
        adapter.journal().entries()[1].status(),
        FederationOperationStatus::Rejected
    );

    let recovered = recover_with_roster_for_test(
        federation_id,
        member_id_value(1),
        0,
        members,
        2,
        adapter.finalized_items().to_vec(),
    )
    .expect("recover finalized conflict");

    assert_eq!(recovered.journal().entries().len(), 2);
    assert_eq!(
        recovered.journal().entries()[1].status(),
        FederationOperationStatus::Rejected
    );
}

#[test]
fn consensus_runtime_recovers_next_local_unit_index_from_finalized_items() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        FederationConsensusRuntime::for_members(federation_id, members[0], 0, members)
            .expect("adapter");

    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-local-before-restart",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit first local operation");
    let first_unit = adapter
        .next_unit_payload()
        .expect("next first local unit")
        .expect("first local unit present");
    assert_eq!(first_unit.submitting_member, members[0]);
    assert_eq!(first_unit.unit_index, 0);
    adapter
        .finalize_unit(first_unit)
        .expect("finalize first local unit");

    let mut recovered = recover_for_members_for_test(
        federation_id,
        members[0],
        0,
        members,
        adapter.finalized_items().to_vec(),
    )
    .expect("recovered adapter");
    assert_eq!(recovered.next_unit_index(), 1);

    recovered
        .submit_operation(mint_envelope(
            federation_id,
            "quote-local-after-restart",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .expect("submit second local operation");
    let second_unit = recovered
        .next_unit_payload()
        .expect("next second local unit")
        .expect("second local unit present");
    assert_eq!(second_unit.submitting_member, members[0]);
    assert_eq!(second_unit.unit_index, 1);
}

#[test]
fn consensus_runtime_session_rotation_waits_for_pending_local_finality() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        FederationConsensusRuntime::for_members(federation_id, members[0], 0, members)
            .expect("adapter");

    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-local-pending-before-rotation",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit operation");
    let pending_unit = adapter
        .next_unit_payload()
        .expect("next unit")
        .expect("pending unit present");
    assert_eq!(pending_unit.session, 0);
    assert_eq!(adapter.status().pending_finalization_item_count, 1);

    assert!(matches!(
        adapter.begin_session(1),
        Err(FederationError::PendingAlephBftFinalization {
            current_session: 0,
            requested_session: 1,
            pending_units: 1,
            pending_items: 1,
        })
    ));
    assert_eq!(adapter.session(), 0);
    assert_eq!(adapter.next_unit_index(), 1);

    adapter
        .finalize_unit(pending_unit)
        .expect("finalize old-session local unit");
    assert_eq!(adapter.status().pending_finalization_item_count, 0);
    adapter
        .begin_session(1)
        .expect("begin session after finality");
    let status = adapter.status();
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(status.next_unit_finalization_index, 0);
    assert_eq!(status.next_item_index, 1);
}

#[test]
fn consensus_runtime_recovers_next_local_unit_index_per_rotated_session() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        FederationConsensusRuntime::for_members(federation_id, members[0], 0, members)
            .expect("adapter");

    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-session-zero",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit session zero operation");
    let session_zero_unit = adapter
        .next_unit_payload()
        .expect("next session zero unit")
        .expect("session zero unit present");
    assert_eq!(session_zero_unit.session, 0);
    assert_eq!(session_zero_unit.unit_index, 0);
    adapter
        .finalize_unit(session_zero_unit)
        .expect("finalize session zero unit");

    adapter.begin_session(1).expect("begin session one");
    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-session-one",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .expect("submit session one operation");
    let session_one_unit = adapter
        .next_unit_payload()
        .expect("next session one unit")
        .expect("session one unit present");
    assert_eq!(session_one_unit.session, 1);
    assert_eq!(session_one_unit.unit_index, 0);
    adapter
        .finalize_unit(session_one_unit)
        .expect("finalize session one unit");

    let mut recovered = recover_for_members_for_test(
        federation_id,
        members[0],
        1,
        members,
        adapter.finalized_items().to_vec(),
    )
    .expect("recover rotated adapter");
    let status = recovered.status();
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_index, 1);
    assert_eq!(status.next_unit_finalization_index, 1);
    assert_eq!(status.next_item_index, 2);

    recovered
        .submit_operation(mint_envelope(
            federation_id,
            "quote-session-one-after-recovery",
            "03df68495f84c316a440429c7e85e6c4b7335e6bad6c80f8591cc735a49ea0e9f6",
        ))
        .expect("submit recovered session one operation");
    let next_unit = recovered
        .next_unit_payload()
        .expect("next recovered unit")
        .expect("recovered unit present");
    assert_eq!(next_unit.session, 1);
    assert_eq!(next_unit.unit_index, 1);
}

#[test]
fn consensus_runtime_advances_local_unit_index_from_replayed_finalized_unit() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        FederationConsensusRuntime::for_members(federation_id, members[0], 0, members)
            .expect("adapter");

    let replayed_envelope = mint_envelope(
        federation_id,
        "quote-replayed-local-unit",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let replayed_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        members[0],
        0,
        0,
        vec![FederationConsensusItem::operation(replayed_envelope)],
    )
    .expect("replayed local unit");
    assert_eq!(adapter.next_unit_index(), 0);

    adapter
        .finalize_unit(replayed_unit)
        .expect("finalize replayed local unit");
    assert_eq!(adapter.next_unit_index(), 1);

    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-after-replayed-local-unit",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))
        .expect("submit next operation");
    let next_unit = adapter
        .next_unit_payload()
        .expect("next local unit")
        .expect("next local unit present");
    assert_eq!(next_unit.submitting_member, members[0]);
    assert_eq!(next_unit.unit_index, 1);
}

#[test]
fn consensus_runtime_recovery_rejects_corrupted_finalized_items() {
    let federation_id = federation_id(13);
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let other_envelope = mint_envelope(
        federation_id,
        "quote-2",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);
    let unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("unit");
    adapter.finalize_unit(unit).expect("finalized");
    let finalized_items = adapter.finalized_items().to_vec();

    let first_ordered_envelope = mint_envelope(
        federation_id,
        "quote-3",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second_ordered_envelope = mint_envelope(
        federation_id,
        "quote-4",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let ordered_first_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_ordered_envelope),
            FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new(
                "mint",
                "payment_observation",
                json!({"quote": "quote-3", "paid": true}),
            )),
        ],
    )
    .expect("ordered first unit");
    let ordered_second_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(3),
        0,
        0,
        vec![FederationConsensusItem::operation(second_ordered_envelope)],
    )
    .expect("ordered second unit");
    let mut ordered_adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);
    ordered_adapter
        .finalize_unit(ordered_first_unit)
        .expect("first ordered unit finalized");
    ordered_adapter
        .finalize_unit(ordered_second_unit)
        .expect("second ordered unit finalized");
    let ordered_finalized_items = ordered_adapter.finalized_items().to_vec();
    assert_eq!(ordered_finalized_items.len(), 3);

    let mut gap = finalized_items.clone();
    gap[0].accepted_item_index = 7;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            gap,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("non-contiguous accepted item index")
    ));

    let mut mismatched_operation_id = finalized_items.clone();
    mismatched_operation_id[0].operation_id =
        Some(other_envelope.operation_id().expect("other operation id"));
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            mismatched_operation_id,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("operation id does not match")
    ));

    let mut mismatched_body_hash = finalized_items.clone();
    mismatched_body_hash[0].unit_body_hash = FederationAlephBftUnitBodyHash::from_bytes([9; 32]);
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            mismatched_body_hash,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("body hash does not match")
    ));

    let mut non_contiguous_item_index = finalized_items.clone();
    non_contiguous_item_index[0].item_index_in_unit = 1;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            non_contiguous_item_index,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("non-contiguous item index")
    ));

    let mut out_of_order_unit_items = ordered_finalized_items.clone();
    out_of_order_unit_items.swap(0, 1);
    out_of_order_unit_items[0].accepted_item_index = 0;
    out_of_order_unit_items[1].accepted_item_index = 1;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            out_of_order_unit_items,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("accepted order")
    ));

    let mut interleaved_unit_items = ordered_finalized_items.clone();
    interleaved_unit_items.swap(1, 2);
    interleaved_unit_items[1].accepted_item_index = 1;
    interleaved_unit_items[2].accepted_item_index = 2;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            interleaved_unit_items,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("not contiguous in accepted item order")
    ));

    let mut non_advancing_unit_finalization = ordered_finalized_items.clone();
    non_advancing_unit_finalization[2].unit_finalization_index = 0;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            non_advancing_unit_finalization,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("non-contiguous unit finalization index")
    ));

    let mut skipped_unit_finalization = ordered_finalized_items.clone();
    skipped_unit_finalization[2].unit_finalization_index = 7;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            skipped_unit_finalization,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("non-contiguous unit finalization index")
    ));

    let mut impossible_duplicate = finalized_items;
    impossible_duplicate[0].status = FederationAlephBftFinalizedItemStatus::DuplicateOperation;
    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            0,
            impossible_duplicate,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("duplicate operation finalized before")
    ));
}

#[test]
fn consensus_runtime_recovery_rejects_out_of_order_sessions() {
    let federation_id = federation_id(13);
    let mut adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);
    let session_zero_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![module_fact_item("session-zero")],
    )
    .expect("session zero unit");
    adapter
        .finalize_unit(session_zero_unit)
        .expect("session zero finalized");

    adapter.begin_session(1).expect("begin session one");
    let session_one_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(3),
        1,
        0,
        vec![module_fact_item("session-one")],
    )
    .expect("session one unit");
    adapter
        .finalize_unit(session_one_unit)
        .expect("session one finalized");

    let finalized_items = adapter.finalized_items().to_vec();
    assert_eq!(
        finalized_items
            .iter()
            .map(|item| item.session)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );

    recover_raw_for_test(
        federation_id,
        member_id_value(1),
        1,
        finalized_items.clone(),
    )
    .expect("valid retained items recover");

    let mut backwards_sessions = finalized_items;
    backwards_sessions.swap(0, 1);
    backwards_sessions[0].accepted_item_index = 0;
    backwards_sessions[1].accepted_item_index = 1;

    assert!(matches!(
        recover_raw_for_test(
            federation_id,
            member_id_value(1),
            1,
            backwards_sessions,
        ),
        Err(FederationError::AlephBftFinalizedItemRecoveryMismatch { reason })
            if reason.contains("session moved backwards")
    ));
}

#[test]
fn consensus_runtime_rejects_invalid_finalized_units_before_journal_append() {
    let federation = federation_id(13);
    let wrong_federation = federation_id(14);
    let wrong_federation_unit =
        FederationAlephBftUnitPayload::new(wrong_federation, member_id_value(2), 0, 0, Vec::new())
            .expect("wrong federation unit");
    let mut tampered_unit =
        FederationAlephBftUnitPayload::new(federation, member_id_value(2), 0, 0, Vec::new())
            .expect("tampered unit");
    tampered_unit.body_hash = FederationAlephBftUnitBodyHash::from_bytes([3; 32]);
    let mut adapter = FederationConsensusRuntime::new(federation, member_id_value(1), 0);

    assert!(matches!(
        adapter.finalize_unit(wrong_federation_unit),
        Err(FederationError::FederationIdMismatch { .. })
    ));
    assert!(matches!(
        adapter.finalize_unit(tampered_unit),
        Err(FederationError::AlephBftUnitBodyHashMismatch { .. })
    ));
    assert_eq!(adapter.journal().len(), 0);
    assert!(adapter.finalized_items().is_empty());
}

#[test]
fn consensus_runtime_advances_sessions_for_units_and_finalization() {
    let federation_id = federation_id(13);
    let mut adapter = FederationConsensusRuntime::new(federation_id, member_id_value(1), 0);
    adapter.begin_session(1).expect("begin next session");
    assert_eq!(adapter.session(), 1);
    assert_eq!(adapter.next_unit_index(), 0);

    let envelope = mint_envelope(
        federation_id,
        "quote-session-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    adapter
        .submit_operation(envelope)
        .expect("submit operation in session 1");
    let unit = adapter
        .next_unit_payload()
        .expect("next unit")
        .expect("unit present");

    assert_eq!(unit.session, 1);
    assert_eq!(unit.unit_index, 0);
    assert_eq!(unit.operation_ids, vec![operation_id]);
    assert_eq!(adapter.finalize_unit(unit).expect("finalize").len(), 1);
}
