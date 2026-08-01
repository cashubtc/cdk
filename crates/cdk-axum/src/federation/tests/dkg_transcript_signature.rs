use super::*;

#[tokio::test]
async fn private_dkg_transcript_signature_route_accepts_signed_peer_request() {
    let (state, public_config, local_private, peer_private, transcript_hash) =
        durable_dkg_transcript_fixture().await;
    let local_member = local_private.member_id;
    let payload = FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        peer_private.member_id,
        transcript_hash,
        FederationDkgTranscriptSignature::sign(transcript_hash, &peer_private)
            .expect("transcript signature"),
    );
    let request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        4,
        payload,
    );

    let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
        .await
        .expect("transcript signature accepted")
        .0;

    response
        .validate(
            public_config.federation_id,
            local_member,
            FederationRestMessageKind::DkgTranscriptSignature,
            peer_private.member_id,
        )
        .expect("transcript signature ack");
    let duplicate = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        5,
        payload,
    );
    let _ = post_dkg_transcript_signature(State(state.clone()), Json(duplicate))
        .await
        .expect("exact transcript signature accepted");
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&5));
    let signatures = state
        .mint
        .localstore()
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("persisted transcript signatures");
    assert_eq!(signatures, vec![payload]);
}

#[tokio::test]
async fn private_dkg_transcript_signature_route_rejects_conflict_before_sequence_persistence() {
    let (state, public_config, local_private, peer_private, transcript_hash) =
        durable_dkg_transcript_fixture().await;
    let local_member = local_private.member_id;
    let first_payload = FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        peer_private.member_id,
        transcript_hash,
        FederationDkgTranscriptSignature::sign(transcript_hash, &peer_private)
            .expect("first transcript signature"),
    );
    let conflicting_payload = FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        peer_private.member_id,
        transcript_hash,
        FederationDkgTranscriptSignature::sign(transcript_hash, &peer_private)
            .expect("conflicting transcript signature"),
    );
    assert_ne!(
        first_payload.signature, conflicting_payload.signature,
        "randomized transcript signatures should produce a distinct conflict payload"
    );
    let first_request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        4,
        first_payload,
    );
    let conflicting_request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        5,
        conflicting_payload,
    );

    let _ = post_dkg_transcript_signature(State(state.clone()), Json(first_request))
        .await
        .expect("first transcript signature accepted");
    let response = post_dkg_transcript_signature(State(state.clone()), Json(conflicting_request))
        .await
        .expect_err("conflicting transcript signature must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&4));
    let signatures = state
        .mint
        .localstore()
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("persisted transcript signatures");
    assert_eq!(signatures, vec![first_payload]);
}

#[tokio::test]
async fn private_dkg_transcript_signature_route_rejects_before_local_keyset_share() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let payload = dkg_transcript_signature_request(&public_config, &peer_private);
    let request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        payload,
    );

    let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
        .await
        .expect_err("transcript signature must wait for local keyset share");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
    let signatures = state
        .mint
        .localstore()
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("persisted transcript signatures");
    assert!(signatures.is_empty());
}

#[tokio::test]
async fn private_dkg_transcript_signature_route_rejects_wrong_hash_before_sequence() {
    let (state, public_config, local_private, peer_private, expected_hash) =
        durable_dkg_transcript_fixture().await;
    let local_member = local_private.member_id;
    let wrong_hash = FederationDkgTranscriptHash::from_bytes([42; 32]);
    assert_ne!(wrong_hash, expected_hash);
    let payload = FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        peer_private.member_id,
        wrong_hash,
        FederationDkgTranscriptSignature::sign(wrong_hash, &peer_private)
            .expect("wrong transcript signature"),
    );
    let request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        4,
        payload,
    );

    let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
        .await
        .expect_err("wrong transcript hash must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
    let signatures = state
        .mint
        .localstore()
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("persisted transcript signatures");
    assert!(signatures.is_empty());
}

#[tokio::test]
async fn private_dkg_transcript_signature_route_rejects_bad_signature_before_sequence() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    let transcript_hash = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript")
        .transcript_hash;
    let bad_signature =
        cdk::federation::FederationDkgTranscriptSignature::sign(transcript_hash, &local_private)
            .expect("bad transcript signature");
    let payload = FederationDkgTranscriptSignatureRequest::new(
        public_config.federation_id,
        public_config.setup_authorization,
        peer_private.member_id,
        transcript_hash,
        bad_signature,
    );
    let request = signed_dkg_transcript_signature_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        payload,
    );

    let response = post_dkg_transcript_signature(State(state.clone()), Json(request))
        .await
        .expect_err("bad transcript signature must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
    let signatures = state
        .mint
        .localstore()
        .get_federation_dkg_transcript_signatures()
        .await
        .expect("persisted transcript signatures");
    assert!(signatures.is_empty());
}
