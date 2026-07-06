use std::collections::BTreeMap;
use std::str::FromStr;

use serde_json::json;

use super::super::{
    FederationConfig, FederationRestMessageKind, FederationSignedRequest, MemberConfigPrivate,
    MemberConfigPublic, ThresholdParams,
};
use super::*;
use crate::mint_url::MintUrl;
use crate::{
    Amount, BlindedMessage, CurrencyUnit, Id, MintQuoteBolt11Request, MintQuoteBolt11Response,
    MintQuoteRequest, MintQuoteResponse, MintRequest, PublicKey, SecretKey,
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

fn module_fact_item(kind: &str) -> FederationConsensusItem {
    FederationConsensusItem::ModuleFact(FederationConsensusModuleFact::new("test", kind, json!({})))
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
fn consensus_runtime_aleph_bft_reports_status_and_rejects_synchronous_submit() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        member_id(),
    )
    .expect("aleph-bft runtime boundary");

    assert_eq!(
        runtime.backend_kind(),
        FederationConsensusBackendKind::AlephBft
    );
    assert_eq!(
        runtime.peer_status().consensus_backend,
        FederationConsensusBackendKind::AlephBft
    );
    assert!(matches!(
        runtime.submit_envelope(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        )),
        Err(FederationError::ConsensusBackendRequiresExternalFinalization { ref backend })
            if backend == "aleph_bft"
    ));
}

#[test]
fn consensus_runtime_aleph_bft_previews_and_drains_matching_units() {
    let federation_id = federation_id(13);
    let mut runtime = FederationConsensusRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        member_id(),
    )
    .expect("aleph-bft runtime boundary");
    let envelope = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");

    runtime
        .submit_aleph_bft_envelope(envelope)
        .expect("submit AlephBFT envelope");
    let preview = runtime
        .preview_next_aleph_bft_unit_payload()
        .expect("preview")
        .expect("unit present");

    assert_eq!(preview.operation_ids, vec![operation_id]);
    assert_eq!(runtime.queued_len(), 1);

    let drained = runtime
        .drain_next_aleph_bft_unit_payload_matching(&preview)
        .expect("drain matching unit");

    assert_eq!(
        drained.canonical_bytes().expect("drained bytes"),
        preview.canonical_bytes().expect("preview bytes")
    );
    assert_eq!(runtime.queued_len(), 0);
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
fn aleph_bft_data_provider_empty_queue_repeated_polls_do_not_advance() {
    let federation_id = federation_id(13);
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 11);

    for _ in 0..8 {
        assert!(provider
            .preview_next_unit_payload()
            .expect("empty preview")
            .is_none());
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
fn aleph_bft_data_provider_previews_without_draining_and_drains_matching_unit() {
    let federation_id = federation_id(13);
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 0);
    let first = mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = mint_envelope(
        federation_id,
        "quote-2",
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let first_id = first.operation_id().expect("first operation id");
    let second_id = second.operation_id().expect("second operation id");

    provider.submit_operation(first).expect("submit first");
    provider.submit_operation(second).expect("submit second");

    let preview = provider
        .preview_next_unit_payload()
        .expect("preview")
        .expect("unit present");
    let second_preview = provider
        .preview_next_unit_payload()
        .expect("second preview")
        .expect("unit present");

    assert_eq!(preview.operation_ids, vec![first_id, second_id]);
    assert_eq!(
        preview.canonical_bytes().expect("preview bytes"),
        second_preview
            .canonical_bytes()
            .expect("second preview bytes")
    );
    assert_eq!(provider.queued_len(), 2);
    assert_eq!(provider.next_unit_index(), 0);

    let drained = provider
        .drain_next_unit_payload_matching(&preview)
        .expect("drain matching unit");

    assert_eq!(
        drained.canonical_bytes().expect("drained bytes"),
        preview.canonical_bytes().expect("preview bytes")
    );
    assert_eq!(provider.queued_len(), 0);
    assert_eq!(provider.next_unit_index(), 1);
    assert!(provider
        .preview_next_unit_payload()
        .expect("empty preview")
        .is_none());
}

#[test]
fn aleph_bft_data_provider_rejects_mismatched_preview_without_draining() {
    let federation_id = federation_id(13);
    let mut provider = FederationAlephBftUnitDataProvider::new(federation_id, member_id(), 0);
    provider
        .submit_operation(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit");
    let preview = provider
        .preview_next_unit_payload()
        .expect("preview")
        .expect("unit present");
    let mut mismatched = preview.clone();
    mismatched.unit_index = 42;

    assert!(matches!(
        provider.drain_next_unit_payload_matching(&mismatched),
        Err(FederationError::AlephBftUnitProposalMismatch {
            expected_unit_index: 42,
            found_unit_index: 0,
            ..
        })
    ));
    assert_eq!(provider.queued_len(), 1);
    assert_eq!(provider.next_unit_index(), 0);

    let drained = provider
        .drain_next_unit_payload_matching(&preview)
        .expect("drain original preview");
    assert_eq!(drained.unit_index, 0);
    assert_eq!(provider.queued_len(), 0);
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
fn aleph_bft_consensus_adapter_builds_units_and_applies_finalized_operations() {
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
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);

    assert_eq!(
        adapter.submit_operation(first).expect("submit first"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        adapter.submit_operation(second).expect("submit second"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        adapter.backend_kind(),
        FederationConsensusBackendKind::AlephBft
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
    assert_eq!(
        first_entry.operation_outcome_hash,
        Some(first_entry.expected_operation_outcome_hash(
            first_entry.state_digest.expect("first state digest")
        ))
    );
    assert_eq!(adapter.finalized_items().len(), 2);
    assert_eq!(
        adapter
            .finalized_items()
            .iter()
            .map(|item| item.operation_id)
            .collect::<Vec<_>>(),
        vec![Some(first_id), Some(second_id)]
    );
    assert_eq!(
        adapter
            .peer_status(federation_id, member_id_value(1))
            .consensus_backend,
        FederationConsensusBackendKind::AlephBft
    );
}

#[test]
fn aleph_bft_consensus_adapter_marks_duplicate_finalized_operations_idempotently() {
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
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);

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
fn aleph_bft_consensus_adapter_recovers_from_finalized_items() {
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
    let mut adapter = FederationAlephBftConsensusAdapter::for_members(
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
    let recovered_runtime =
        FederationConsensusBackendAdapterRuntime::aleph_bft_from_finalized_items(
            federation_id,
            [member_id_value(1), member_id_value(2), member_id_value(3)],
            2,
            member_id_value(1),
            0,
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

    let mut recovered =
        FederationAlephBftConsensusAdapter::recover_for_members_from_finalized_items(
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
fn aleph_bft_consensus_adapter_recovery_rejects_corrupted_finalized_items() {
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
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);
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
    let mut ordered_adapter =
        FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
fn aleph_bft_consensus_adapter_recovery_rejects_out_of_order_sessions() {
    let federation_id = federation_id(13);
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);
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

    FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
        FederationAlephBftConsensusAdapter::recover_from_finalized_items(
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
fn aleph_bft_consensus_adapter_rejects_invalid_finalized_units_before_journal_append() {
    let federation = federation_id(13);
    let wrong_federation = federation_id(14);
    let wrong_federation_unit =
        FederationAlephBftUnitPayload::new(wrong_federation, member_id_value(2), 0, 0, Vec::new())
            .expect("wrong federation unit");
    let mut tampered_unit =
        FederationAlephBftUnitPayload::new(federation, member_id_value(2), 0, 0, Vec::new())
            .expect("tampered unit");
    tampered_unit.body_hash = FederationAlephBftUnitBodyHash::from_bytes([3; 32]);
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation, member_id_value(1), 0);

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
fn aleph_bft_consensus_adapter_advances_sessions_for_units_and_finalization() {
    let federation_id = federation_id(13);
    let mut adapter = FederationAlephBftConsensusAdapter::new(federation_id, member_id_value(1), 0);
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
fn backend_adapter_runtime_dispatches_selected_aleph_bft_backend() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("aleph-bft adapter runtime");
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
        adapter.backend_kind(),
        FederationConsensusBackendKind::AlephBft
    );
    assert_eq!(
        adapter
            .submit_operation(first_envelope)
            .expect("submit first"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(
        adapter
            .submit_operation(second_envelope)
            .expect("submit second"),
        FederationConsensusItemAdmission::Accepted
    );
    assert_eq!(adapter.queued_len(), 2);
    assert_eq!(adapter.pending_finalization_len(), 0);

    let preview = adapter
        .preview_next_aleph_bft_unit_payload()
        .expect("preview unit")
        .expect("unit");
    assert_eq!(preview.operation_ids, vec![first, second]);
    assert_eq!(adapter.queued_len(), 2);

    let unit = adapter
        .drain_next_aleph_bft_unit_payload_matching(&preview)
        .expect("drain matching unit");
    assert_eq!(unit.operation_ids, vec![first, second]);
    assert_eq!(adapter.queued_len(), 0);

    let responses = adapter
        .finalize_aleph_bft_unit(unit)
        .expect("finalize unit");

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0].operation_id, first);
    assert_eq!(responses[1].operation_id, second);
    assert_eq!(
        adapter
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .len(),
        2
    );
}

#[test]
fn backend_adapter_runtime_aleph_bft_generic_dispatch_fails_closed() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("aleph-bft adapter runtime");
    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-1",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit");

    assert!(matches!(
        adapter.dispatch_and_finalize(10),
        Err(FederationError::ConsensusBackendRequiresExternalFinalization { ref backend })
            if backend == "aleph_bft"
    ));
    assert_eq!(adapter.queued_len(), 1);
    assert_eq!(adapter.pending_finalization_len(), 0);
}

#[test]
fn backend_adapter_runtime_aleph_bft_validates_roster_before_queueing() {
    let members = [member_id_value(1), member_id_value(2), member_id_value(2)];

    assert!(matches!(
        FederationConsensusBackendAdapterRuntime::with_backend(
            FederationConsensusBackendKind::AlephBft,
            federation_id(13),
            members,
            2,
            member_id_value(1),
        ),
        Err(FederationError::DuplicateMember { member_id }) if member_id == member_id_value(2)
    ));

    let err = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id(13),
        [member_id_value(1), member_id_value(2), member_id_value(3)],
        2,
        member_id_value(4),
    )
    .expect_err("local member must be in roster");

    assert!(matches!(
        err,
        FederationError::UnknownMember { member_id }
            if member_id == member_id_value(4)
    ));
}

#[test]
fn backend_adapter_runtime_aleph_bft_rejects_finalized_unit_from_unknown_member() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("aleph-bft adapter runtime");
    let unknown_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(4),
        0,
        0,
        vec![FederationConsensusItem::operation(mint_envelope(
            federation_id,
            "quote-unknown",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))],
    )
    .expect("unknown member unit");

    assert!(matches!(
        adapter.finalize_aleph_bft_unit(unknown_unit),
        Err(FederationError::UnknownMember { member_id })
            if member_id == member_id_value(4)
    ));
    assert!(adapter
        .aleph_bft_finalized_items()
        .expect("finalized items")
        .is_empty());

    let known_unit = FederationAlephBftUnitPayload::new(
        federation_id,
        member_id_value(2),
        0,
        0,
        vec![FederationConsensusItem::operation(mint_envelope(
            federation_id,
            "quote-known",
            "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
        ))],
    )
    .expect("known member unit");
    let responses = adapter
        .finalize_aleph_bft_unit(known_unit)
        .expect("known member unit finalizes");

    assert_eq!(responses.len(), 1);
    assert_eq!(
        adapter
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .len(),
        1
    );
}

#[test]
fn backend_adapter_runtime_reports_aleph_bft_status_across_queue_and_finalization() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let mut adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::AlephBft,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("aleph-bft adapter runtime");
    let initial = adapter.aleph_bft_status().expect("initial status");

    assert_eq!(
        initial.consensus_backend,
        FederationConsensusBackendKind::AlephBft
    );
    assert_eq!(initial.federation_id, federation_id);
    assert_eq!(initial.local_member, members[0]);
    assert_eq!(initial.session, 0);
    assert_eq!(initial.next_unit_index, 0);
    assert_eq!(initial.next_unit_finalization_index, 0);
    assert_eq!(initial.next_item_index, 0);
    assert_eq!(initial.queued_item_count, 0);
    assert_eq!(initial.finalized_item_count, 0);
    assert_eq!(initial.accepted_operation_count, 0);
    assert_eq!(initial.latest_accepted_operation_index, None);
    assert_eq!(initial.allowed_member_count, Some(3));

    adapter
        .submit_operation(mint_envelope(
            federation_id,
            "quote-status",
            "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
        ))
        .expect("submit operation");
    let queued = adapter.aleph_bft_status().expect("queued status");
    assert_eq!(queued.queued_item_count, 1);
    assert_eq!(queued.next_unit_index, 0);

    let unit = adapter
        .next_aleph_bft_unit_payload()
        .expect("next unit")
        .expect("unit");
    let pulled = adapter.aleph_bft_status().expect("pulled status");
    assert_eq!(pulled.queued_item_count, 0);
    assert_eq!(pulled.next_unit_index, 1);
    assert_eq!(pulled.next_unit_finalization_index, 0);
    assert_eq!(pulled.finalized_item_count, 0);

    adapter
        .finalize_aleph_bft_unit(unit)
        .expect("finalize unit");
    let finalized = adapter.aleph_bft_status().expect("finalized status");
    assert_eq!(finalized.next_unit_index, 1);
    assert_eq!(finalized.next_unit_finalization_index, 1);
    assert_eq!(finalized.next_item_index, 1);
    assert_eq!(finalized.finalized_item_count, 1);
    assert_eq!(finalized.accepted_operation_count, 1);
    assert_eq!(finalized.latest_accepted_operation_index, Some(0));
}

#[test]
fn backend_adapter_runtime_rejects_aleph_bft_status_for_in_memory_backend() {
    let federation_id = federation_id(13);
    let members = [member_id_value(1), member_id_value(2), member_id_value(3)];
    let adapter = FederationConsensusBackendAdapterRuntime::with_backend(
        FederationConsensusBackendKind::InMemory,
        federation_id,
        members,
        2,
        members[0],
    )
    .expect("in-memory adapter runtime");

    assert!(matches!(
        adapter.aleph_bft_status(),
        Err(FederationError::UnsupportedConsensusBackend { ref backend })
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
fn dummy_consensus_applies_catch_up_response_after_session_outcome_quorum() {
    let (config, private_members) = consensus_config_three();
    let federation_id = config.federation_id;
    let mut local = InMemoryFederationConsensus::new(federation_id, private_members[2].member_id);
    let mut peer = InMemoryFederationConsensus::new(federation_id, private_members[0].member_id);
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    peer.submit_envelope(mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    ))
    .expect("peer mint submit");

    let request = local.catch_up_request();
    let response = peer
        .catch_up_checked(&request)
        .expect("catch up")
        .with_signed_session_outcome(&private_members[0])
        .expect("signed catch up");
    let second_outcome = peer
        .catch_up_checked(&request)
        .expect("second catch up")
        .with_signed_session_outcome(&private_members[1])
        .expect("second signed catch up")
        .session_outcome
        .expect("second outcome");

    let verified = local
        .apply_catch_up_response_with_session_outcome_quorum(&response, &config, &[second_outcome])
        .expect("quorum verified catch up");

    assert_eq!(verified.applied_entries.len(), 2);
    assert_eq!(verified.session_outcome_quorum.required, 2);
    assert_eq!(verified.session_outcome_quorum.operation_count, 2);
    assert_eq!(verified.session_outcome_quorum.signers.len(), 2);
    assert_eq!(local.journal().len(), 2);
    assert_eq!(
        local.peer_status().latest_order_digest,
        peer.peer_status().latest_order_digest
    );
}

#[test]
fn dummy_consensus_rejects_quorum_checked_catch_up_without_enough_signers() {
    let (config, private_members) = consensus_config_three();
    let federation_id = config.federation_id;
    let mut local = InMemoryFederationConsensus::new(federation_id, private_members[2].member_id);
    let mut peer = InMemoryFederationConsensus::new(federation_id, private_members[0].member_id);
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");

    let response = peer
        .catch_up_checked(&local.catch_up_request())
        .expect("catch up")
        .with_signed_session_outcome(&private_members[0])
        .expect("signed catch up");

    assert!(matches!(
        local.apply_catch_up_response_with_session_outcome_quorum(&response, &config, &[]),
        Err(FederationError::SessionOutcomeQuorumNotReached {
            required: 2,
            found: 1
        })
    ));
    assert_eq!(local.journal().len(), 0);
}

#[test]
fn dummy_consensus_rejects_quorum_checked_catch_up_with_mismatched_outcome() {
    let (config, private_members) = consensus_config_three();
    let federation_id = config.federation_id;
    let mut local = InMemoryFederationConsensus::new(federation_id, private_members[2].member_id);
    let mut peer = InMemoryFederationConsensus::new(federation_id, private_members[0].member_id);
    peer.submit_envelope(mint_quote_envelope(federation_id))
        .expect("peer quote submit");
    peer.submit_envelope(mint_envelope(
        federation_id,
        "quote-1",
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    ))
    .expect("peer mint submit");

    let request = local.catch_up_request();
    let response = peer
        .catch_up_checked(&request)
        .expect("catch up")
        .with_signed_session_outcome(&private_members[0])
        .expect("signed catch up");
    let mismatched_outcome = peer
        .catch_up_checked(&FederationJournalCatchUpRequest::new(1))
        .expect("mismatched catch up")
        .with_signed_session_outcome(&private_members[1])
        .expect("mismatched signed catch up")
        .session_outcome
        .expect("mismatched outcome");

    assert!(matches!(
        local.apply_catch_up_response_with_session_outcome_quorum(
            &response,
            &config,
            &[mismatched_outcome],
        ),
        Err(FederationError::SessionOutcomeMismatch { .. })
    ));
    assert_eq!(local.journal().len(), 0);
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
