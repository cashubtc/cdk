use super::*;

#[tokio::test]
async fn private_operation_route_accepts_signed_peer_request() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        envelope.clone(),
    );

    let response = post_operation(State(state.clone()), Json(request))
        .await
        .expect("operation response");

    assert_eq!(
        response.0.operation_id,
        envelope.operation_id().expect("operation id")
    );
    assert_eq!(consensus_operation_count(&state).await, 1);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("persisted journal");
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].operation_id, response.0.operation_id);
    assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
}

#[tokio::test]
async fn private_operation_route_rejects_oversized_request_before_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        oversized_payment_observation_envelope(public_config.federation_id),
    );

    let response = post_operation(State(state.clone()), Json(request))
        .await
        .expect_err("oversized request must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(consensus_operation_count(&state).await, 0);
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_rejects_tampered_payload_before_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let first = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope(
        public_config.federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let mut request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        first,
    );
    request.payload = FederationSubmitOperationRequest::new(second);

    let response = post_operation(State(state.clone()), Json(request))
        .await
        .expect_err("tampered request must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(consensus_operation_count(&state).await, 0);
}

#[tokio::test]
async fn private_operation_route_rejects_unknown_sender_before_mutation() {
    let (state, public_config, _) = federation_route_state().await;
    let unknown_private = private_member(3);
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let request = signed_operation_request(
        &public_config,
        &unknown_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        envelope,
    );

    let response = post_operation(State(state.clone()), Json(request))
        .await
        .expect_err("unknown sender must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(consensus_operation_count(&state).await, 0);
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_rejects_kind_mismatch_before_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let envelope = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::Status,
        envelope,
    );

    let response = post_operation(State(state.clone()), Json(request))
        .await
        .expect_err("wrong message kind must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(consensus_operation_count(&state).await, 0);
    let sequences = state
        .mint
        .localstore()
        .get_federation_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_rejects_stale_sequence_before_second_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let first = restore_envelope(
        public_config.federation_id,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let second = restore_envelope(
        public_config.federation_id,
        "025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7",
    );
    let first_request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        first,
    );
    let second_request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        second,
    );

    let _ = post_operation(State(state.clone()), Json(first_request))
        .await
        .expect("first operation accepted");
    let response = post_operation(State(state.clone()), Json(second_request))
        .await
        .expect_err("stale sequence must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(consensus_operation_count(&state).await, 1);
}
