use super::*;

#[tokio::test]
async fn private_operation_route_accepts_signed_peer_request() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let envelope = test_operation_envelope_for_state(&state, "accepted private operation");
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].operation_id, response.0.operation_id);
    assert!(
        sequences.is_empty(),
        "operation IDs, not durable REST sequences, provide operation idempotency"
    );
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_rejects_tampered_payload_before_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let first = test_operation_envelope_for_state(&state, "signed private operation");
    let second = test_operation_envelope_for_state(&state, "tampered private operation");
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
    let envelope = test_operation_envelope_for_state(&state, "unknown sender operation");
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_rejects_kind_mismatch_before_mutation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let envelope = test_operation_envelope_for_state(&state, "wrong message kind operation");
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
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_operation_route_replay_is_idempotent_by_operation_id() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let envelope = test_operation_envelope_for_state(&state, "replayed operation");
    let request = signed_operation_request(
        &public_config,
        &peer_private,
        1,
        FederationRestMessageKind::SubmitOperation,
        envelope,
    );

    let first = post_operation(State(state.clone()), Json(request.clone()))
        .await
        .expect("first operation accepted")
        .0;
    let replay = post_operation(State(state.clone()), Json(request))
        .await
        .expect("operation replay returns its durable outcome")
        .0;

    assert_eq!(replay.operation_id, first.operation_id);
    assert!(matches!(
        replay.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(consensus_operation_count(&state).await, 1);
}

#[tokio::test]
async fn private_operation_route_allows_sequence_reuse_by_operation_id_policy() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let first = test_operation_envelope_for_state(&state, "first sequenced operation");
    let second = test_operation_envelope_for_state(&state, "stale sequenced operation");
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

    let first_response = post_operation(State(state.clone()), Json(first_request))
        .await
        .expect("first operation accepted");
    let second_response = post_operation(State(state.clone()), Json(second_request))
        .await
        .expect("operation-ID policy accepts a distinct operation despite sequence reuse");

    assert_ne!(
        second_response.0.operation_id,
        first_response.0.operation_id
    );
    assert_eq!(consensus_operation_count(&state).await, 2);
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable DKG sequences")
        .is_empty());
}
