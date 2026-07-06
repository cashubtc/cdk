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
