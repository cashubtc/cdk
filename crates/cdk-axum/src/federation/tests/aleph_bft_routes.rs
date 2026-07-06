use super::*;

async fn aleph_bft_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_backend(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
        FederationConsensusBackendKind::AlephBft,
    )
    .await;

    (state, public_config, peer_private)
}

fn aleph_bft_unit(
    state: &MintState,
    submitting_member: MemberId,
    quote_id: &QuoteId,
) -> FederationAlephBftUnitPayload {
    FederationAlephBftUnitPayload::new(
        state
            .mint
            .federation_config()
            .expect("federation config")
            .federation_id(),
        submitting_member,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            state, quote_id,
        ))],
    )
    .expect("AlephBFT unit")
}

fn signed_aleph_bft_finalized_unit_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    kind: FederationRestMessageKind,
    unit: FederationAlephBftUnitPayload,
) -> FederationSignedRequest<FederationAlephBftFinalizedUnitRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        kind,
        FederationAlephBftFinalizedUnitRequest::new(unit),
        private_config,
    )
    .expect("signed AlephBFT finalized-unit request")
}

#[tokio::test]
async fn local_status_includes_aleph_bft_adapter_counters() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    {
        let consensus = state
            .federation_consensus
            .as_ref()
            .expect("consensus handle");
        consensus
            .lock()
            .await
            .submit_aleph_bft_envelope(envelope)
            .expect("queued AlephBFT operation");
    }

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response");
    let status = response.0;
    let aleph_bft_status = status.aleph_bft_status.expect("AlephBFT status");

    assert_eq!(status.pending_mempool_items, 1);
    assert_eq!(
        aleph_bft_status.consensus_backend,
        FederationConsensusBackendKind::AlephBft
    );
    assert!(!status.sync_ready);
    assert_eq!(status.sync_state, "halted");
    assert!(
        status
            .sync_message
            .contains("public federation submissions are disabled"),
        "unexpected sync message: {}",
        status.sync_message
    );
    assert_eq!(aleph_bft_status.local_member, local_member);
    assert_eq!(aleph_bft_status.session, 0);
    assert_eq!(aleph_bft_status.queued_item_count, 1);
    assert_eq!(aleph_bft_status.pending_finalization_item_count, 0);
    assert_eq!(aleph_bft_status.finalized_item_count, 0);
    assert_eq!(aleph_bft_status.accepted_operation_count, 0);
    assert_eq!(aleph_bft_status.allowed_member_count, Some(2));
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_persists_applies_and_records_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, peer_private.member_id, &quote_id);
    let operation_ids = unit.operation_ids.clone();
    let request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit,
    );

    let response = post_aleph_bft_finalized_unit(State(state.clone()), Json(request))
        .await
        .expect("AlephBFT finalized-unit response")
        .0;

    assert_eq!(response.federation_id, public_config.federation_id);
    assert_eq!(response.operation_ids, operation_ids);
    assert_eq!(response.responses.len(), 1);
    assert_eq!(response.responses[0].operation_id, operation_ids[0]);

    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("persisted finalized items");
    assert_eq!(finalized_items.len(), 1);
    assert_eq!(finalized_items[0].operation_id, Some(operation_ids[0]));

    let journal_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("persisted journal");
    assert_eq!(journal_entries.len(), 1);
    assert_eq!(journal_entries[0].operation_id, operation_ids[0]);

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("materialized mint quote");
    assert_eq!(quote.id, quote_id);

    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_accepts_exact_duplicate_replay_idempotently() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, peer_private.member_id, &quote_id);
    let operation_ids = unit.operation_ids.clone();
    let first_request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit.clone(),
    );
    let second_request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        2,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit,
    );

    let first_response = post_aleph_bft_finalized_unit(State(state.clone()), Json(first_request))
        .await
        .expect("first AlephBFT finalized-unit response")
        .0;
    let second_response = post_aleph_bft_finalized_unit(State(state.clone()), Json(second_request))
        .await
        .expect("duplicate AlephBFT finalized-unit response")
        .0;

    assert_eq!(first_response.operation_ids, operation_ids);
    assert_eq!(first_response.responses.len(), 1);
    assert_eq!(second_response.operation_ids, operation_ids);
    assert_eq!(second_response.responses.len(), 1);
    assert!(matches!(
        first_response.responses[0].outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));
    assert!(matches!(
        second_response.responses[0].outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(
        second_response.responses[0].operation_id,
        first_response.responses[0].operation_id
    );

    let finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("persisted finalized items");
    assert_eq!(finalized_items.len(), 1);
    assert_eq!(finalized_items[0].operation_id, Some(operation_ids[0]));

    let journal_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("persisted journal");
    assert_eq!(journal_entries.len(), 1);
    assert_eq!(journal_entries[0].operation_id, operation_ids[0]);

    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&2));
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_rejects_kind_mismatch_before_mutation() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, peer_private.member_id, &quote_id);
    let request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::Status,
        unit,
    );

    let response = post_aleph_bft_finalized_unit(State(state.clone()), Json(request))
        .await
        .expect_err("wrong kind must fail");

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("persisted finalized items")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences")
        .is_empty());
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_rejects_sender_unit_member_mismatch() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, local_member, &quote_id);
    let request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit,
    );

    let response = post_aleph_bft_finalized_unit(State(state.clone()), Json(request))
        .await
        .expect_err("sender/unit member mismatch must fail");

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("persisted finalized items")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after rejected member mismatch")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences after rejected member mismatch")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected member mismatch")
        .is_none());
    let status = state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .peer_status();
    assert_eq!(status.operation_count, 0);
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_rejects_empty_unit_before_mutation() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        Vec::new(),
    )
    .expect("empty AlephBFT unit");
    let request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit,
    );

    let response = post_aleph_bft_finalized_unit(State(state.clone()), Json(request))
        .await
        .expect_err("empty finalized unit must fail");

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after empty unit")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after empty unit")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences after empty unit")
        .is_empty());
    let status = state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .peer_status();
    assert_eq!(status.operation_count, 0);
}

#[tokio::test]
async fn private_aleph_bft_finalized_unit_route_rejects_storage_conflict_before_mutation() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let quote_id = QuoteId::new();
    let conflicting_quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, peer_private.member_id, &quote_id);
    let conflicting_unit = aleph_bft_unit(&state, peer_private.member_id, &conflicting_quote_id);
    let mut conflicting_adapter = FederationAlephBftConsensusAdapter::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("conflicting AlephBFT adapter");
    conflicting_adapter
        .finalize_unit(conflicting_unit)
        .expect("conflicting finalized unit");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict transaction");
    tx.add_federation_aleph_bft_finalized_items(conflicting_adapter.finalized_items())
        .await
        .expect("persist conflicting finalized item");
    tx.commit().await.expect("commit conflict transaction");
    let request = signed_aleph_bft_finalized_unit_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftFinalizedUnit,
        unit,
    );

    let response = post_aleph_bft_finalized_unit(State(state.clone()), Json(request))
        .await
        .expect_err("conflicting finalized unit must fail");

    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after conflict")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after conflict")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("sequences after conflict")
        .is_empty());
    let status = state
        .federation_consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .peer_status();
    assert_eq!(status.operation_count, 0);
}

#[tokio::test]
async fn federation_peer_client_posts_signed_aleph_bft_finalized_unit() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, local_private.member_id, &quote_id);
    let envelope = mint_quote_envelope(&state, &quote_id);
    let response = FederationAlephBftFinalizedUnitResponse::new(
        public_config.federation_id,
        &unit,
        vec![FederationSubmitOperationResponse::from_outcome(
            InMemoryFederationJournal::new()
                .append(envelope)
                .expect("journal append"),
        )],
    );
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response.clone());
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let request = FederationAlephBftFinalizedUnitRequest::new(unit.clone());

    let received = client
        .submit_aleph_bft_finalized_unit(peer_member, &request)
        .await
        .expect("submit AlephBFT finalized unit");

    assert_eq!(received.unit_body_hash, unit.body_hash);
    let requests = transport.aleph_bft_finalized_unit_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].kind,
        FederationRestMessageKind::AlephBftFinalizedUnit
    );
    assert_eq!(requests[0].payload.unit.body_hash, unit.body_hash);
    requests[0]
        .verify(&public_config, peer_private.member_id)
        .expect("signed request verifies");
}

#[tokio::test]
async fn federation_peer_client_rejects_aleph_bft_unknown_member_before_signing() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, local_private.member_id, &quote_id);
    let request = FederationAlephBftFinalizedUnitRequest::new(unit);
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let mut unknown_peer = public_config
        .member(peer_private.member_id)
        .expect("peer member")
        .clone();
    unknown_peer.member_id = member_id(99);

    let err = client
        .submit_aleph_bft_finalized_unit(&unknown_peer, &request)
        .await
        .expect_err("unknown peer should be rejected");

    assert!(
        err.to_string()
            .contains("is not in the configured member roster"),
        "unexpected error: {err}"
    );
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());
}

#[tokio::test]
async fn federation_peer_client_rejects_aleph_bft_mismatched_member_before_signing() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, local_private.member_id, &quote_id);
    let request = FederationAlephBftFinalizedUnitRequest::new(unit);
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let mut mismatched_peer = public_config
        .member(peer_private.member_id)
        .expect("peer member")
        .clone();
    mismatched_peer.identity_public_key = SecretKey::from_slice(&[99; 32])
        .expect("mismatched identity secret")
        .public_key();

    let err = client
        .submit_aleph_bft_finalized_unit(&mismatched_peer, &request)
        .await
        .expect_err("mismatched peer should be rejected");

    assert!(
        err.to_string()
            .contains("does not match the configured member roster"),
        "unexpected error: {err}"
    );
    assert!(transport.aleph_bft_finalized_unit_requests().is_empty());
}

#[tokio::test]
async fn rest_peer_client_rejects_aleph_bft_response_with_mismatched_outcome_operation_id() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let quote_id = QuoteId::new();
    let unit = aleph_bft_unit(&state, local_private.member_id, &quote_id);
    let expected_operation_id = *unit.operation_ids.first().expect("unit operation id");
    let other_quote_id = QuoteId::new();
    let other_envelope = mint_quote_envelope(&state, &other_quote_id);
    let mut submit_response = FederationSubmitOperationResponse::from_outcome(
        InMemoryFederationJournal::new()
            .append(other_envelope)
            .expect("journal append"),
    );
    let found_operation_id = submit_response.operation_id;
    assert_ne!(expected_operation_id, found_operation_id);
    submit_response.operation_id = expected_operation_id;
    let response = FederationAlephBftFinalizedUnitResponse::new(
        public_config.federation_id,
        &unit,
        vec![submit_response],
    );
    let transport =
        MockFederationTransport::default().with_aleph_bft_finalized_unit_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let request = FederationAlephBftFinalizedUnitRequest::new(unit.clone());

    let err = client
        .submit_aleph_bft_finalized_unit(peer_member, &request)
        .await
        .expect_err("mismatched embedded outcome should be rejected");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::SubmitOperationResponseMismatch { expected, found })
            if *expected == expected_operation_id && *found == found_operation_id
    ));
    let requests = transport.aleph_bft_finalized_unit_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].payload.unit.body_hash, unit.body_hash);
}
