use std::str::FromStr;

use serde_json::json;

use super::*;
use crate::{
    Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey,
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

fn mint_quote_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        super::super::FederationOperation::MintQuote(super::super::FederationMintQuoteOperation {
            quote: "quote-1".to_string(),
            request: MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: "quote-1".to_string(),
                request: "invoice-quote-1".to_string(),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: crate::MintQuoteState::Unpaid,
                expiry: None,
                pubkey: None,
            }),
        }),
    )
}

fn oversized_mint_quote_envelope(federation_id: FederationId) -> FederationOperationEnvelope {
    let mut envelope = mint_quote_envelope(federation_id);
    let super::super::FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("mint quote operation");
    };
    let crate::MintQuoteRequest::Bolt11(request) = &mut operation.request else {
        panic!("bolt11 request");
    };
    request.description = Some("x".repeat(super::super::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
    envelope
}

#[test]
fn dummy_consensus_accepts_duplicates_and_rejects_conflicts() {
    let federation_id = federation_id(13);
    let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
    let first = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let duplicate = first.clone();
    let conflict = mint_envelope(
        federation_id,
        "quote-1",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );

    let first_response = consensus.submit_envelope(first).expect("submit");
    let duplicate_response = consensus.submit_envelope(duplicate).expect("duplicate");

    assert_eq!(first_response.operation_id, duplicate_response.operation_id);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert!(matches!(
        consensus.submit_envelope(conflict),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[test]
fn consensus_engine_trait_accepts_operations_in_submit_order() {
    let federation_id = federation_id(13);
    let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
    let engine: &mut dyn FederationConsensusEngine = &mut consensus;

    let first = engine
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit first");
    let second = engine
        .submit_envelope(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit second");

    assert_eq!(engine.federation_id(), federation_id);
    assert_eq!(engine.member_id(), member_id());
    assert_eq!(first.outcome.entry().index, 0);
    assert_eq!(second.outcome.entry().index, 1);
    assert_eq!(engine.peer_status().operation_count, 2);
    assert_eq!(engine.state_digest(), engine.peer_status().state_digest);
    assert_eq!(
        engine.journal().entries()[0].operation_id,
        first.operation_id
    );
    assert_eq!(
        engine.journal().entries()[1].operation_id,
        second.operation_id
    );
}

#[test]
fn consensus_runtime_replays_persisted_in_memory_entries() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::in_memory(federation_id, member_id());
    let first = runtime
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit first");
    let second = runtime
        .submit_envelope(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit second");
    runtime
        .update_status(second.operation_id, FederationOperationStatus::Applied)
        .expect("update status");

    let replayed = FederationConsensusRuntime::in_memory_from_entries(
        federation_id,
        member_id(),
        runtime.journal().entries().to_vec(),
    )
    .expect("replay runtime");

    assert_eq!(replayed.peer_status().operation_count, 2);
    assert_eq!(
        replayed.peer_status().latest_order_digest,
        runtime.peer_status().latest_order_digest
    );
    assert_eq!(replayed.state_digest(), runtime.state_digest());
    assert_eq!(
        replayed
            .journal()
            .entry_by_operation_id(first.operation_id)
            .expect("first entry")
            .index,
        0
    );
    assert_eq!(
        replayed
            .journal()
            .entry_by_operation_id(second.operation_id)
            .expect("second entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[test]
fn consensus_runtime_replay_rejects_future_operation_versions() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::in_memory(federation_id, member_id());
    runtime
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit first");
    let mut entries = runtime.journal().entries().to_vec();
    entries[0].envelope.version = super::super::FEDERATION_OPERATION_VERSION + 1;

    assert!(matches!(
        FederationConsensusRuntime::in_memory_from_entries(federation_id, member_id(), entries),
        Err(FederationError::OperationVersionMismatch {
            expected: super::super::FEDERATION_OPERATION_VERSION,
            found,
        }) if found == super::super::FEDERATION_OPERATION_VERSION + 1
    ));
}

#[test]
fn consensus_backend_kind_parses_known_values_and_rejects_unknown() {
    assert_eq!(
        "in_memory"
            .parse::<FederationConsensusBackendKind>()
            .expect("in-memory backend"),
        FederationConsensusBackendKind::InMemory
    );
    assert_eq!(
        "memory"
            .parse::<FederationConsensusBackendKind>()
            .expect("memory alias"),
        FederationConsensusBackendKind::InMemory
    );
    assert_eq!(
        "aleph-bft"
            .parse::<FederationConsensusBackendKind>()
            .expect("aleph alias"),
        FederationConsensusBackendKind::AlephBft
    );
    assert!("raft"
        .parse::<FederationConsensusBackendKind>()
        .expect_err("unknown backend should fail")
        .contains("Unknown federation consensus backend"));
}

#[test]
fn consensus_runtime_rejects_unavailable_backend_kind() {
    let err = FederationConsensusRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id(13),
        member_id(),
    )
    .expect_err("aleph-bft backend is not implemented yet");

    assert!(matches!(
        err,
        FederationError::UnsupportedConsensusBackend { ref backend }
            if backend == "aleph_bft"
    ));
}

#[test]
fn consensus_runtime_replays_entries_with_selected_backend() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::with_backend(
        FederationConsensusBackendKind::InMemory,
        federation_id,
        member_id(),
    )
    .expect("in-memory runtime");
    runtime
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit first");

    let replayed = FederationConsensusRuntime::from_entries_with_backend(
        FederationConsensusBackendKind::InMemory,
        federation_id,
        member_id(),
        runtime.journal().entries().to_vec(),
    )
    .expect("replay runtime");

    assert_eq!(
        replayed.backend_kind(),
        FederationConsensusBackendKind::InMemory
    );
    assert_eq!(
        replayed.peer_status().consensus_backend,
        FederationConsensusBackendKind::InMemory
    );
    assert_eq!(replayed.peer_status().operation_count, 1);
    assert_eq!(
        replayed.peer_status().latest_order_digest,
        runtime.peer_status().latest_order_digest
    );
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
    let payload =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 0, 0, Vec::new())
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
fn aleph_bft_finalization_handler_advances_to_later_sessions() {
    let federation_id = federation_id(13);
    let mut handler = FederationAlephBftFinalizationHandler::new(federation_id);
    let first =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 0, 0, Vec::new())
            .expect("first payload");
    handler.finalize_next_unit(first).expect("first finalized");

    handler.begin_session(1).expect("begin next session");
    assert_eq!(handler.session(), 1);
    assert_eq!(handler.next_unit_finalization_index(), 0);
    assert_eq!(handler.next_item_index(), 0);

    let next =
        FederationAlephBftUnitPayload::new(federation_id, member_id_value(2), 1, 0, Vec::new())
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
fn multi_node_engine_converges_with_reordered_duplicate_delayed_messages() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut network =
        InMemoryFederationConsensusNetwork::new(federation_id, members, 2).expect("network");
    let first = network
        .submit(
            members[0],
            mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ),
        )
        .expect("submit first");
    let second = network
        .submit(
            members[1],
            mint_envelope(
                federation_id,
                "quote-2",
                "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
            ),
        )
        .expect("submit second");
    let third = network
        .submit(
            members[2],
            mint_envelope(
                federation_id,
                "quote-3",
                "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
            ),
        )
        .expect("submit third");

    assert!(network.delay_next_proposal());
    assert!(network.duplicate_next_proposal());
    let responses = network.deliver_all().expect("deliver all");

    assert_eq!(responses.len(), 4);
    let expected_order = vec![second, third, first];
    for member in members {
        assert_eq!(
            network
                .member_status(member)
                .expect("member status")
                .operation_count,
            3
        );
        assert_eq!(
            network
                .accepted_operation_ids(member)
                .expect("accepted ids"),
            expected_order
        );
    }
    assert!(network.all_members_converged());
}

#[test]
fn multi_node_engine_requires_quorum_and_catches_up_offline_member() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut network =
        InMemoryFederationConsensusNetwork::new(federation_id, members, 2).expect("network");
    network
        .set_online(members[1], false)
        .expect("member 2 offline");
    network
        .set_online(members[2], false)
        .expect("member 3 offline");
    network
        .submit(
            members[0],
            mint_envelope(
                federation_id,
                "quote-1",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ),
        )
        .expect("submit");

    assert!(matches!(
        network.deliver_next(),
        Err(FederationError::ConsensusQuorumUnavailable {
            reachable: 1,
            required: 2
        })
    ));
    assert_eq!(network.pending_len(), 1);
    assert_eq!(
        network
            .member_status(members[0])
            .expect("member 1 status")
            .operation_count,
        0
    );

    network
        .set_online(members[1], true)
        .expect("member 2 online");
    network.deliver_next().expect("deliver with quorum");
    assert_eq!(
        network
            .member_status(members[0])
            .expect("member 1 status")
            .operation_count,
        1
    );
    assert_eq!(
        network
            .member_status(members[2])
            .expect("member 3 status")
            .operation_count,
        0
    );
    assert!(!network.all_members_converged());

    network
        .catch_up_member_from_peer(members[2], members[0])
        .expect("catch up member 3");
    assert!(network.all_members_converged());
}

#[test]
fn multi_node_engine_catches_up_offline_member_across_default_limit() {
    let federation_id = federation_id(31);
    let members = (1..=4).map(member_id_value).collect::<Vec<_>>();
    let public_keys = [
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
    ];
    let mut network = InMemoryFederationConsensusNetwork::new(federation_id, members.clone(), 3)
        .expect("network");

    network
        .set_online(members[3], false)
        .expect("member 4 offline");

    let mut expected_order = Vec::new();
    for index in 0..105 {
        let quote = format!("offline-catch-up-{index}");
        let sender = members[index % 3];
        expected_order.push(
            network
                .submit(
                    sender,
                    mint_envelope(
                        federation_id,
                        &quote,
                        public_keys[index % public_keys.len()],
                    ),
                )
                .expect("submit operation"),
        );
    }

    let delivered = network.deliver_all().expect("deliver operations");
    assert_eq!(delivered.len(), expected_order.len());
    for member in members.iter().take(3).copied() {
        assert_eq!(
            network
                .accepted_operation_ids(member)
                .expect("online accepted ids"),
            expected_order
        );
    }
    assert_eq!(
        network
            .member_status(members[3])
            .expect("offline member status")
            .operation_count,
        0
    );

    let applied = network
        .catch_up_member_from_peer(members[3], members[0])
        .expect("catch up offline member");

    assert_eq!(applied.len(), expected_order.len());
    assert_eq!(
        network
            .accepted_operation_ids(members[3])
            .expect("caught-up accepted ids"),
        expected_order
    );
    assert!(network.all_members_converged());
}

#[test]
fn multi_node_engine_scales_to_seven_members_with_threshold_partition_catch_up() {
    let federation_id = federation_id(14);
    let members = (1..=7).map(member_id_value).collect::<Vec<_>>();
    let public_keys = [
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        "0238b95dfe74856e3e3d122fef8ca3a674cce8f22388e361c1ee8b9d52386cd61c",
    ];
    let mut network = InMemoryFederationConsensusNetwork::new(federation_id, members.clone(), 5)
        .expect("network");

    network
        .set_online(members[5], false)
        .expect("member 6 offline");
    network
        .set_online(members[6], false)
        .expect("member 7 offline");

    let mut operation_ids = Vec::new();
    for index in 0..6 {
        let quote = format!("quote-{index}");
        let sender = members[index % 5];
        operation_ids.push(
            network
                .submit(
                    sender,
                    mint_envelope(
                        federation_id,
                        &quote,
                        public_keys[index % public_keys.len()],
                    ),
                )
                .expect("submit operation"),
        );
    }

    assert!(network.delay_next_proposal());
    assert!(network.duplicate_next_proposal());
    assert!(network.delay_next_proposal());
    let responses = network.deliver_all().expect("deliver at threshold");
    let mut expected_order = vec![
        operation_ids[2],
        operation_ids[3],
        operation_ids[4],
        operation_ids[5],
        operation_ids[0],
        operation_ids[1],
    ];

    assert_eq!(responses.len(), 7);
    for member in members.iter().take(5).copied() {
        assert_eq!(
            network
                .accepted_operation_ids(member)
                .expect("accepted ids"),
            expected_order
        );
    }
    for member in members.iter().skip(5).copied() {
        assert_eq!(
            network
                .member_status(member)
                .expect("offline member status")
                .operation_count,
            0
        );
    }
    assert!(!network.all_members_converged());

    network
        .set_online(members[4], false)
        .expect("member 5 offline");
    let stalled_operation = network
        .submit(
            members[0],
            mint_envelope(
                federation_id,
                "quote-stalled",
                "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
            ),
        )
        .expect("submit stalled operation");
    assert!(matches!(
        network.deliver_next(),
        Err(FederationError::ConsensusQuorumUnavailable {
            reachable: 4,
            required: 5
        })
    ));
    assert_eq!(network.pending_len(), 1);

    network
        .set_online(members[4], true)
        .expect("member 5 online");
    let response = network
        .deliver_next()
        .expect("deliver after quorum recovers")
        .expect("finalized response");
    expected_order.push(stalled_operation);

    assert_eq!(response.operation_id, stalled_operation);
    assert_eq!(network.pending_len(), 0);
    for member in members.iter().take(5).copied() {
        assert_eq!(
            network
                .accepted_operation_ids(member)
                .expect("accepted ids"),
            expected_order
        );
    }

    network
        .set_online(members[5], true)
        .expect("member 6 online");
    network
        .set_online(members[6], true)
        .expect("member 7 online");
    network
        .catch_up_member_from_peer(members[5], members[0])
        .expect("catch up member 6");
    network
        .catch_up_member_from_peer(members[6], members[4])
        .expect("catch up member 7");

    for member in members {
        assert_eq!(
            network
                .accepted_operation_ids(member)
                .expect("accepted ids"),
            expected_order
        );
    }
    assert!(network.all_members_converged());
}

#[test]
fn backend_adapter_dispatches_queued_operations_to_finalized_order() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
            .expect("adapter");
    let first_envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let first = first_envelope.operation_id().expect("first operation id");
    let second_envelope = mint_envelope(
        federation_id,
        "quote-2",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let second = second_envelope.operation_id().expect("second operation id");

    assert_eq!(
        adapter
            .submit_operation(first_envelope.clone())
            .expect("submit first"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        adapter
            .submit_operation(first_envelope)
            .expect("duplicate first"),
        FederationConsensusItemAdmission::DuplicateOperation
    );
    assert_eq!(
        adapter
            .submit_operation(second_envelope)
            .expect("submit second"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(adapter.queued_len(), 2);

    let responses = adapter
        .dispatch_and_finalize(10)
        .expect("dispatch and finalize");

    assert_eq!(responses.len(), 2);
    assert_eq!(adapter.queued_len(), 0);
    assert_eq!(adapter.network_pending_len(), 0);
    for member in members {
        assert_eq!(
            adapter
                .accepted_operation_ids(member)
                .expect("accepted ids"),
            vec![first, second]
        );
    }
    assert!(adapter.all_members_converged());
}

#[test]
fn backend_adapter_trait_dispatches_in_memory_operations() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
            .expect("adapter");
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");

    {
        let backend: &mut dyn FederationConsensusBackendAdapter = &mut adapter;
        assert_eq!(
            backend.backend_kind(),
            FederationConsensusBackendKind::InMemory
        );
        assert_eq!(
            backend.submit_operation(envelope).expect("submit"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(backend.queued_len(), 1);
        assert_eq!(backend.pending_finalization_len(), 0);

        let responses = backend
            .dispatch_and_finalize(10)
            .expect("dispatch and finalize");

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].operation_id, operation_id);
        assert_eq!(backend.queued_len(), 0);
        assert_eq!(backend.pending_finalization_len(), 0);
    }

    assert!(adapter.all_members_converged());
}

#[test]
fn backend_adapter_runtime_dispatches_selected_in_memory_backend() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::InMemory,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("adapter runtime");
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    assert_eq!(
        adapter.backend_kind(),
        FederationConsensusBackendKind::InMemory
    );
    assert_eq!(
        adapter.submit_operation(envelope).expect("submit"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(adapter.queued_len(), 1);
    assert_eq!(adapter.dispatch_queued_operations(10).expect("dispatch"), 1);
    assert_eq!(adapter.queued_len(), 0);
    assert_eq!(adapter.pending_finalization_len(), 1);

    let responses = adapter.finalize_dispatched_operations().expect("finalize");

    assert_eq!(responses.len(), 1);
    assert_eq!(adapter.pending_finalization_len(), 0);
}

#[test]
fn backend_adapter_runtime_rejects_unavailable_backend_kind() {
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];

    let err = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id(13),
        members,
        2,
        members[0],
    )
    .expect_err("aleph-bft adapter is not implemented yet");

    assert!(matches!(
        err,
        FederationError::UnsupportedConsensusBackend { ref backend }
            if backend == "aleph_bft"
    ));
}

#[test]
fn backend_adapter_keeps_dispatched_operation_pending_without_quorum() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
            .expect("adapter");
    adapter
        .set_online(members[1], false)
        .expect("member 2 offline");
    adapter
        .set_online(members[2], false)
        .expect("member 3 offline");
    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit");

    assert_eq!(adapter.dispatch_queued_operations(10).expect("dispatch"), 1);
    assert_eq!(adapter.queued_len(), 0);
    assert_eq!(adapter.network_pending_len(), 1);
    assert!(matches!(
        adapter.finalize_dispatched_operations(),
        Err(FederationError::ConsensusQuorumUnavailable {
            reachable: 1,
            required: 2
        })
    ));
    assert_eq!(adapter.network_pending_len(), 1);
}

#[test]
fn backend_adapter_keeps_local_queue_when_proposer_is_offline() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter =
        InMemoryFederationConsensusAdapter::new(federation_id, members, 2, members[0])
            .expect("adapter");
    adapter
        .set_online(members[0], false)
        .expect("local member offline");
    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit");

    assert!(adapter.dispatch_queued_operations(10).is_err());
    assert_eq!(adapter.queued_len(), 1);
    assert_eq!(adapter.network_pending_len(), 0);
}

#[test]
fn dummy_consensus_rejects_wrong_federation() {
    let mut consensus = InMemoryFederationConsensus::new(federation_id(13), member_id());
    let envelope = mint_quote_envelope(federation_id(14));

    assert!(matches!(
        consensus.submit_operation(FederationSubmitOperationRequest::new(envelope)),
        Err(FederationError::OperationFederationMismatch { .. })
    ));
}

#[test]
fn dummy_consensus_rejects_oversized_operation_before_journal_append() {
    let federation_id = federation_id(13);
    let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
    let envelope = oversized_mint_quote_envelope(federation_id);

    assert!(matches!(
        consensus.submit_envelope(envelope),
        Err(FederationError::OperationEnvelopeTooLarge { .. })
    ));
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[test]
fn dummy_consensus_serves_peer_status_and_catch_up() {
    let federation_id = federation_id(13);
    let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
    consensus
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit quote");
    consensus
        .submit_envelope(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit mint");

    let status = consensus.peer_status();
    let request = FederationJournalCatchUpRequest::new(1);
    let response = consensus.catch_up(&request);

    assert_eq!(status.federation_id, federation_id);
    assert_eq!(status.member_id, member_id());
    assert_eq!(status.operation_count, 2);
    assert_eq!(response.from_index, 1);
    assert_eq!(response.entries.len(), 1);
    assert!(response.complete);
    assert_eq!(response.latest_order_digest, status.latest_order_digest);
}

#[test]
fn dummy_consensus_applies_peer_catch_up_response() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    let quote = mint_quote_envelope(federation_id);
    let mint = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    local
        .submit_envelope(quote.clone())
        .expect("local quote submit");
    peer.submit_envelope(quote).expect("peer quote submit");
    let mint_response = peer.submit_envelope(mint).expect("peer mint submit");
    peer.update_status(
        mint_response.operation_id,
        FederationOperationStatus::Applied,
    )
    .expect("peer status update");

    let request = local.catch_up_request();
    let response = peer.catch_up_checked(&request).expect("catch up");
    let applied = local
        .apply_catch_up_response(&response)
        .expect("apply catch up");

    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].operation_id, mint_response.operation_id);
    assert_eq!(applied[0].status, FederationOperationStatus::Accepted);
    assert_eq!(
        local.peer_status().latest_order_digest,
        peer.peer_status().latest_order_digest
    );
}

#[test]
fn dummy_consensus_rejects_known_digest_mismatch() {
    let federation_id = federation_id(13);
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    let request = FederationJournalCatchUpRequest::new(1)
        .with_known_order_digest(super::super::FederationJournalDigest::genesis());

    assert!(matches!(
        peer.catch_up_checked(&request),
        Err(FederationError::CatchUpDigestMismatch { .. })
    ));
}

#[test]
fn dummy_consensus_rejects_wrong_federation_catch_up_response() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    let mut response = peer
        .catch_up_checked(&local.catch_up_request())
        .expect("catch up");
    response.federation_id = super::super::FederationId::from_bytes([14; 32]);

    assert!(matches!(
        local.apply_catch_up_response(&response),
        Err(FederationError::CatchUpFederationMismatch { .. })
    ));
}

#[test]
fn dummy_consensus_rejects_tampered_catch_up_operation_id() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    let mut response = peer
        .catch_up_checked(&local.catch_up_request())
        .expect("catch up");
    response.entries[0].operation_id = FederationOperationId::from_bytes([99; 32]);

    assert!(matches!(
        local.apply_catch_up_response(&response),
        Err(FederationError::CatchUpOperationIdMismatch { .. })
    ));
}

#[test]
fn dummy_consensus_rejects_future_operation_version_catch_up_response() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    let mut response = peer
        .catch_up_checked(&local.catch_up_request())
        .expect("catch up");
    response.entries[0].envelope.version = super::super::FEDERATION_OPERATION_VERSION + 1;

    assert!(matches!(
        local.apply_catch_up_response(&response),
        Err(FederationError::OperationVersionMismatch {
            expected: super::super::FEDERATION_OPERATION_VERSION,
            found,
        }) if found == super::super::FEDERATION_OPERATION_VERSION + 1
    ));
    assert_eq!(local.peer_status().operation_count, 0);
}

#[test]
fn dummy_consensus_rejects_tampered_catch_up_digest_chain() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let mut peer = InMemoryFederationConsensus::new(federation_id, member_id());
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    let mut response = peer
        .catch_up_checked(&local.catch_up_request())
        .expect("catch up");
    response.entries[0].previous_order_digest =
        super::super::FederationJournalDigest::from_bytes([88; 32]);

    assert!(matches!(
        local.apply_catch_up_response(&response),
        Err(FederationError::CatchUpDigestMismatch { .. })
    ));
}

#[test]
fn dummy_consensus_rejects_empty_incomplete_catch_up_response() {
    let federation_id = federation_id(13);
    let mut local = InMemoryFederationConsensus::new(federation_id, member_id());
    let response = FederationJournalCatchUpResponse {
        version: super::super::FEDERATION_REST_API_VERSION,
        federation_id,
        from_index: 0,
        entries: Vec::new(),
        latest_index: Some(0),
        latest_order_digest: super::super::FederationJournalDigest::genesis(),
        state_digest: FederationStateDigest::genesis(),
        session_outcome: None,
        complete: false,
    };

    assert!(matches!(
        local.apply_catch_up_response(&response),
        Err(FederationError::CatchUpNoProgress { index: 0 })
    ));
}

#[test]
fn dummy_consensus_updates_application_status() {
    let federation_id = federation_id(13);
    let mut consensus = InMemoryFederationConsensus::new(federation_id, member_id());
    let response = consensus
        .submit_envelope(mint_quote_envelope(federation_id))
        .expect("submit");
    let updated = consensus
        .update_status(response.operation_id, FederationOperationStatus::Applied)
        .expect("update status");

    assert_eq!(updated.status, FederationOperationStatus::Applied);
}
