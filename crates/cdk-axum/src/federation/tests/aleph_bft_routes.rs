use cdk::federation::FederationAlephBftActivityRequest;

use super::*;

async fn aleph_bft_route_state() -> (MintState, FederationConfig, MemberConfigPrivate) {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    (state, public_config, peer_private)
}

fn signed_aleph_bft_network_message_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    kind: FederationRestMessageKind,
    session_id: u64,
    message: Vec<u8>,
) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        kind,
        FederationAlephBftNetworkMessageRequest::new(session_id, message),
        private_config,
    )
    .expect("signed AlephBFT network-message request")
}

fn signed_aleph_bft_activity_request(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    sequence: u64,
    session_id: u64,
    operation_id: FederationOperationId,
) -> FederationSignedRequest<FederationAlephBftActivityRequest> {
    FederationSignedRequest::new_signed(
        public_config.federation_id,
        member_id(1),
        sequence,
        FederationRestMessageKind::AlephBftActivity,
        FederationAlephBftActivityRequest::new(session_id, operation_id),
        private_config,
    )
    .expect("signed AlephBFT activity request")
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
            .federation
            .consensus
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
    assert!(!status.aleph_bft_live_session_active);
    assert!(!status.aleph_bft_network_inbound_active);
    assert!(!status.sync_ready);
    assert_eq!(status.sync_state, "awaiting_quorum");
    assert!(
        status
            .sync_message
            .contains("awaiting a quorum-compatible peer view"),
        "unexpected sync message: {}",
        status.sync_message
    );
    assert_eq!(status.peer_status.aleph_bft_session, 0);
    assert_eq!(aleph_bft_status.local_member, local_member);
    assert_eq!(aleph_bft_status.session, 0);
    assert_eq!(aleph_bft_status.queued_item_count, 1);
    assert_eq!(aleph_bft_status.pending_finalization_item_count, 0);
    assert_eq!(aleph_bft_status.finalized_item_count, 0);
    assert_eq!(aleph_bft_status.accepted_operation_count, 0);
    assert_eq!(aleph_bft_status.allowed_member_count, Some(2));
}

#[tokio::test]
async fn private_aleph_bft_activity_route_wakes_live_session_and_preserves_rejected_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &state,
        FederationPeerSyncConfig::default(),
        CdkAlephBftNetworkPumpConfig::default(),
        MockFederationTransport::default(),
    )
    .await
    .expect("start AlephBFT live session")
    .expect("live session handle");
    let operation_id = mint_quote_envelope(&state, &QuoteId::new())
        .operation_id()
        .expect("activity operation id");
    let wrong_session =
        signed_aleph_bft_activity_request(&public_config, &peer_private, 1, 1, operation_id);

    let response = post_aleph_bft_activity(State(state.clone()), Json(wrong_session))
        .await
        .expect_err("wrong activity session must fail");
    let detail = response_error_detail(response).await;
    assert!(
        detail.contains("Federation AlephBFT unit session mismatch"),
        "unexpected detail: {detail}"
    );

    let request =
        signed_aleph_bft_activity_request(&public_config, &peer_private, 1, 0, operation_id);
    let response = post_aleph_bft_activity(State(state.clone()), Json(request.clone()))
        .await
        .expect("activity response")
        .0;
    assert_eq!(response.federation_id, public_config.federation_id);
    assert_eq!(response.session_id, 0);
    assert_eq!(response.operation_id, operation_id);
    assert!(handle.activity_is_active());

    let replay = post_aleph_bft_activity(State(state), Json(request))
        .await
        .expect_err("activity request replay must fail");
    let detail = response_error_detail(replay).await;
    assert!(
        detail.contains("stale sequence"),
        "unexpected detail: {detail}"
    );
}

#[tokio::test]
async fn aleph_bft_activity_wake_finalizes_five_member_operation_after_idle() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let session = start_five_member_aleph_bft_live_session(
        FederationAlephBftLiveSessionOptions::new(pump_config).with_max_round(500),
    )
    .await
    .expect("start five-member live session");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let latency = latency_reservation.install();

    let operation_id = submit_latency_probe_mint_quote(&session.states[0]).await;
    for state in &session.states {
        wait_for_applied_operation(state, operation_id).await;
    }

    let events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&events, LATENCY_EXPECTED_STAGES);
    let submit_to_yield = latency_stage_delta(
        &events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::DataProviderYielded,
    )
    .expect("submit-to-provider-yield latency");
    assert!(
        submit_to_yield < Duration::from_secs(5),
        "five-member idle wake took {submit_to_yield:?}: {}",
        format_latency_events(&events)
    );
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_accepts_signed_peer_request() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let (mut network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let activity_hint = CdkAlephBftActivityHint::default();
    *state.federation.aleph_bft_network_inbound.write().await = Some(Arc::new(
        CdkAlephBftNetworkInboundHandle::with_activity_hint(0, &handle, activity_hint.clone()),
    ));
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        42u64.encode(),
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect("AlephBFT network-message response")
        .0;

    assert_eq!(response.federation_id, public_config.federation_id);
    assert_eq!(response.session_id, 0);
    assert_eq!(network.next_event().await, Some(42));
    assert!(activity_hint.is_active());
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences");
    assert!(
        sequences.is_empty(),
        "AlephBFT gossip should not persist liveness-only sequence watermarks"
    );
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_same_sequence_equivocation() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let (mut network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    *state.federation.aleph_bft_network_inbound.write().await =
        Some(Arc::new(CdkAlephBftNetworkInboundHandle::new(0, &handle)));
    let first_request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        42u64.encode(),
    );
    let equivocated_request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        43u64.encode(),
    );

    let _ = post_aleph_bft_network_message(State(state.clone()), Json(first_request))
        .await
        .expect("first AlephBFT network message accepted");
    assert_eq!(network.next_event().await, Some(42));
    let response = post_aleph_bft_network_message(State(state.clone()), Json(equivocated_request))
        .await
        .expect_err("same-sequence equivocated message must fail");
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("stale sequence"),
        "unexpected detail: {detail}"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), network.next_event())
            .await
            .is_err(),
        "same-sequence equivocated payload must not be delivered"
    );
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences after equivocation");
    assert!(
        sequences.is_empty(),
        "AlephBFT gossip sequence watermarks should remain in memory only"
    );
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_kind_mismatch_before_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::Status,
        0,
        vec![1, 2, 3, 4],
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect_err("kind mismatch must fail");
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("message kind mismatch"),
        "unexpected detail: {detail}"
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after rejection")
        .is_empty());
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_missing_inbound_before_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        42u64.encode(),
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect_err("missing inbound receiver must fail");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("AlephBFT network message receiver is not available"),
        "unexpected detail: {detail}"
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after rejection")
        .is_empty());
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_bad_scale_before_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let (_network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    *state.federation.aleph_bft_network_inbound.write().await =
        Some(Arc::new(CdkAlephBftNetworkInboundHandle::new(0, &handle)));
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        vec![1, 2, 3],
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect_err("bad SCALE message must fail");
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("Invalid federation AlephBFT network message"),
        "unexpected detail: {detail}"
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after rejection")
        .is_empty());
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_trailing_scale_bytes_before_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let (mut network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    *state.federation.aleph_bft_network_inbound.write().await =
        Some(Arc::new(CdkAlephBftNetworkInboundHandle::new(0, &handle)));
    let mut message = 42u64.encode();
    message.extend_from_slice(&[99]);
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        message,
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect_err("SCALE message with trailing bytes must fail");
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("trailing bytes after SCALE network message"),
        "unexpected detail: {detail}"
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after rejection")
        .is_empty());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), network.next_event())
            .await
            .is_err(),
        "trailing-byte rejection must not deliver the decoded message"
    );
}

#[tokio::test]
async fn private_aleph_bft_network_message_route_rejects_session_mismatch_before_sequence() {
    let (state, public_config, peer_private) = aleph_bft_route_state().await;
    let (_network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    *state.federation.aleph_bft_network_inbound.write().await =
        Some(Arc::new(CdkAlephBftNetworkInboundHandle::new(2, &handle)));
    let request = signed_aleph_bft_network_message_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        0,
        42u64.encode(),
    );

    let response = post_aleph_bft_network_message(State(state.clone()), Json(request))
        .await
        .expect_err("session mismatch must fail");
    let detail = response_error_detail(response).await;

    assert!(
        detail.contains("Federation AlephBFT unit session mismatch"),
        "unexpected detail: {detail}"
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after rejection")
        .is_empty());
}

#[tokio::test]
async fn federation_peer_client_posts_signed_aleph_bft_network_message() {
    let (public_config, local_private, peer_private) = transport_config();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 3);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let request = FederationAlephBftNetworkMessageRequest::new(3, vec![9, 8, 7, 6]);

    let received = client
        .submit_aleph_bft_network_message(peer_member, &request)
        .await
        .expect("submit AlephBFT network message");

    assert_eq!(received.session_id, 3);
    let requests = transport.aleph_bft_network_message_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].kind,
        FederationRestMessageKind::AlephBftNetworkMessage
    );
    assert_eq!(
        requests[0].payload.message_bytes().expect("bytes"),
        vec![9, 8, 7, 6]
    );
    requests[0]
        .verify(&public_config, peer_private.member_id)
        .expect("signed request verifies");
}

#[tokio::test]
async fn federation_peer_client_rejects_aleph_bft_network_unknown_member_before_signing() {
    let (public_config, local_private, peer_private) = transport_config();
    let request = FederationAlephBftNetworkMessageRequest::new(3, vec![9, 8, 7, 6]);
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
        .submit_aleph_bft_network_message(&unknown_peer, &request)
        .await
        .expect_err("unknown peer should be rejected");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::UnknownMember { member_id: found })
            if *found == unknown_peer.member_id
    ));
    assert!(transport.aleph_bft_network_message_requests().is_empty());
}

#[tokio::test]
async fn federation_peer_client_rejects_aleph_bft_network_response_session_mismatch() {
    let (public_config, local_private, peer_private) = transport_config();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 4);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let request = FederationAlephBftNetworkMessageRequest::new(3, vec![9, 8, 7, 6]);

    let err = client
        .submit_aleph_bft_network_message(peer_member, &request)
        .await
        .expect_err("mismatched session should be rejected");

    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::AlephBftUnitSessionMismatch { expected, found })
            if *expected == 3 && *found == 4
    ));
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 1);
}
