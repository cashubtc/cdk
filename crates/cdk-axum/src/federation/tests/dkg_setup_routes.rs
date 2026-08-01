use super::*;

#[tokio::test]
async fn private_dkg_setup_routes_accept_signed_peer_requests() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let commitment_payload = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    let expected_commitment = commitment_payload.commitment;
    let reveal_payload = dkg_reveal_request(&public_config, peer_private.member_id, 21);
    let expected_reveal = reveal_payload.reveal.clone();
    let secret_share_payload =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
    let expected_secret_share = secret_share_payload.clone();
    let commitment_request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        commitment_payload,
    );
    let reveal_request = signed_dkg_reveal_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        reveal_payload,
    );
    let secret_share_request = signed_dkg_secret_share_request(
        &public_config,
        &peer_private,
        local_member,
        3,
        secret_share_payload,
    );

    let commitment_response = post_dkg_commitment(State(state.clone()), Json(commitment_request))
        .await
        .expect("commitment accepted")
        .0;
    let reveal_response = post_dkg_reveal(State(state.clone()), Json(reveal_request))
        .await
        .expect("reveal accepted")
        .0;
    let secret_share_response =
        post_dkg_secret_share(State(state.clone()), Json(secret_share_request))
            .await
            .expect("secret share accepted")
            .0;

    commitment_response
        .validate(
            public_config.federation_id,
            local_member,
            FederationRestMessageKind::DkgCommitment,
            peer_private.member_id,
        )
        .expect("commitment ack");
    reveal_response
        .validate(
            public_config.federation_id,
            local_member,
            FederationRestMessageKind::DkgReveal,
            peer_private.member_id,
        )
        .expect("reveal ack");
    secret_share_response
        .validate(
            public_config.federation_id,
            local_member,
            FederationRestMessageKind::DkgSecretShare,
            peer_private.member_id,
        )
        .expect("secret-share ack");
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
    let commitments = state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("persisted commitments");
    let reveals = state
        .mint
        .localstore()
        .get_federation_dkg_reveals()
        .await
        .expect("persisted reveals");
    let secret_shares = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("persisted secret-share contributions");
    assert_eq!(commitments, vec![expected_commitment]);
    assert_eq!(reveals, vec![expected_reveal]);
    assert_eq!(secret_shares, vec![expected_secret_share]);
}

#[tokio::test]
async fn private_dkg_commitment_route_rejects_inactive_ceremony_before_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let mut payload = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    payload.commitment.ceremony_id = FederationDkgCeremonyId::from_bytes([56; 32]);
    let request =
        signed_dkg_commitment_request(&public_config, &peer_private, local_member, 1, payload);

    let response = post_dkg_commitment(State(state.clone()), Json(request))
        .await
        .expect_err("inactive ceremony commitment must fail");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    assert!(String::from_utf8_lossy(&body).contains("another ceremony"));
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());
}

#[tokio::test]
async fn private_dkg_setup_route_rejects_when_no_ceremony_is_active() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state.federation.set_dkg_auto_start_config_for_test(None);
    let request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        dkg_commitment_request(&public_config, peer_private.member_id, 21),
    );

    let response = post_dkg_commitment(State(state.clone()), Json(request))
        .await
        .expect_err("inactive DKG route must fail");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());
}

#[tokio::test]
async fn private_dkg_reveal_route_rejects_inactive_keyset_policy_before_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let mut payload = dkg_reveal_request(&public_config, peer_private.member_id, 21);
    payload.reveal.keyset_spec.input_fee_ppk = 1;
    let request =
        signed_dkg_reveal_request(&public_config, &peer_private, local_member, 1, payload);

    let response = post_dkg_reveal(State(state.clone()), Json(request))
        .await
        .expect_err("inactive keyset policy must fail");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    assert!(String::from_utf8_lossy(&body).contains("active ceremony"));
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_reveals()
        .await
        .expect("reveals")
        .is_empty());
}

#[tokio::test]
async fn private_dkg_router_rejects_oversized_signed_body_before_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        dkg_commitment_request(&public_config, peer_private.member_id, 21),
    );
    let mut body = serde_json::to_vec(&request).expect("signed request JSON");
    body.resize(FEDERATION_MAX_DKG_REQUEST_BODY_BYTES + 1, b' ');

    let response = create_federation_router()
        .with_state(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::POST)
                .uri("/dkg/commitments")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body))
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());
}

#[tokio::test]
async fn saturated_dkg_member_admission_does_not_block_another_member() {
    let (state, public_config, local_private, first_peer, second_peer) =
        federation_route_state_three_with_local_private().await;
    let admissions = state
        .federation
        .dkg_member_admission()
        .expect("DKG member admission");
    let held_permit = admissions
        .get(&first_peer.member_id)
        .expect("first peer admission")
        .in_flight
        .clone()
        .try_acquire_owned()
        .expect("hold first peer admission");
    let first_request = signed_dkg_commitment_request(
        &public_config,
        &first_peer,
        local_private.member_id,
        1,
        dkg_commitment_request(&public_config, first_peer.member_id, 21),
    );
    let second_request = signed_dkg_commitment_request(
        &public_config,
        &second_peer,
        local_private.member_id,
        1,
        dkg_commitment_request(&public_config, second_peer.member_id, 31),
    );

    let saturated = post_dkg_commitment(State(state.clone()), Json(first_request.clone()))
        .await
        .expect_err("saturated member must fail fast");
    assert_eq!(saturated.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = post_dkg_commitment(State(state.clone()), Json(second_request))
        .await
        .expect("independent member remains admissible");

    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences");
    assert!(!sequences.contains_key(&first_peer.member_id));
    assert_eq!(sequences.get(&second_peer.member_id), Some(&1));
    drop(held_permit);

    let _ = post_dkg_commitment(State(state.clone()), Json(first_request))
        .await
        .expect("saturated member can retry the same request");
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences after retry");
    assert_eq!(sequences.get(&first_peer.member_id), Some(&1));
    assert_eq!(sequences.get(&second_peer.member_id), Some(&1));
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_dkg_commitments()
            .await
            .expect("commitments")
            .len(),
        2
    );
}

#[tokio::test]
async fn sustained_dkg_member_rate_limit_preserves_fairness_and_retry() {
    let (mut state, public_config, local_private, first_peer, second_peer) =
        federation_route_state_three_with_local_private().await;
    state.federation.set_dkg_member_admission_for_test(Some(
        crate::federation_dkg_member_admission(
            &public_config,
            1,
            std::time::Duration::from_millis(10),
        ),
    ));
    let first_commitment = dkg_commitment_request(&public_config, first_peer.member_id, 21);
    let first_request = signed_dkg_commitment_request(
        &public_config,
        &first_peer,
        local_private.member_id,
        1,
        first_commitment.clone(),
    );
    let limited_retry = signed_dkg_commitment_request(
        &public_config,
        &first_peer,
        local_private.member_id,
        2,
        first_commitment.clone(),
    );
    let second_request = signed_dkg_commitment_request(
        &public_config,
        &second_peer,
        local_private.member_id,
        1,
        dkg_commitment_request(&public_config, second_peer.member_id, 31),
    );

    let _ = post_dkg_commitment(State(state.clone()), Json(first_request))
        .await
        .expect("first member request");
    let limited = post_dkg_commitment(State(state.clone()), Json(limited_retry.clone()))
        .await
        .expect_err("same member must exhaust its request bucket");
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = post_dkg_commitment(State(state.clone()), Json(second_request))
        .await
        .expect("another member retains an independent request bucket");

    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences before refill");
    assert_eq!(sequences.get(&first_peer.member_id), Some(&1));
    assert_eq!(sequences.get(&second_peer.member_id), Some(&1));

    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    let _ = post_dkg_commitment(State(state.clone()), Json(limited_retry))
        .await
        .expect("same request is retryable after token refill");
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences after refill");
    assert_eq!(sequences.get(&first_peer.member_id), Some(&2));
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_dkg_commitments()
            .await
            .expect("commitments")
            .len(),
        2
    );
}

#[tokio::test]
async fn active_dkg_route_fails_closed_without_member_admission() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state.federation.set_dkg_member_admission_for_test(None);
    let request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        dkg_commitment_request(&public_config, peer_private.member_id, 21),
    );

    let response = post_dkg_commitment(State(state.clone()), Json(request))
        .await
        .expect_err("active DKG without member admission must fail closed");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("member sequences")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("commitments")
        .is_empty());
}

#[tokio::test]
async fn private_dkg_setup_routes_accept_exact_duplicates_and_advance_sequence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let commitment = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    let reveal = dkg_reveal_request(&public_config, peer_private.member_id, 21);
    let secret_share =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);

    for sequence in 1..=2 {
        let request = signed_dkg_commitment_request(
            &public_config,
            &peer_private,
            local_member,
            sequence,
            commitment,
        );
        let _ = post_dkg_commitment(State(state.clone()), Json(request))
            .await
            .expect("exact commitment accepted");
    }
    for sequence in 3..=4 {
        let request = signed_dkg_reveal_request(
            &public_config,
            &peer_private,
            local_member,
            sequence,
            reveal.clone(),
        );
        let _ = post_dkg_reveal(State(state.clone()), Json(request))
            .await
            .expect("exact reveal accepted");
    }
    for sequence in 5..=6 {
        let request = signed_dkg_secret_share_request(
            &public_config,
            &peer_private,
            local_member,
            sequence,
            secret_share.clone(),
        );
        let _ = post_dkg_secret_share(State(state.clone()), Json(request))
            .await
            .expect("exact secret share accepted");
    }

    let db = state.mint.localstore();
    assert_eq!(
        db.get_federation_dkg_member_sequences()
            .await
            .expect("member sequences")
            .get(&peer_private.member_id),
        Some(&6)
    );
    assert_eq!(
        db.get_federation_dkg_commitments()
            .await
            .expect("commitments")
            .len(),
        1
    );
    assert_eq!(
        db.get_federation_dkg_reveals()
            .await
            .expect("reveals")
            .len(),
        1
    );
    assert_eq!(
        db.get_federation_dkg_secret_share_contributions()
            .await
            .expect("secret-share contributions")
            .len(),
        1
    );
}

#[tokio::test]
async fn completed_secret_share_duplicate_skips_cpu_derivation() {
    let (state, _, _, peer_private, _) = durable_dkg_transcript_fixture().await;
    let db = state.mint.localstore();
    let contribution = db
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("secret-share contributions")
        .into_iter()
        .find(|contribution| contribution.sender == peer_private.member_id)
        .expect("peer contribution");
    let keyset_shares_before = db
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("member keyset shares");
    let cpu_permit = crate::FEDERATION_DKG_CPU_ADMISSION
        .acquire()
        .await
        .expect("DKG CPU permit");

    for sequence in 4..14 {
        tokio::time::timeout(
            Duration::from_millis(100),
            state.persist_federation_dkg_secret_share_contribution(
                &contribution,
                peer_private.member_id,
                sequence,
            ),
        )
        .await
        .expect("exact duplicate does not wait for CPU derivation")
        .expect("exact duplicate persists sequence");
    }
    drop(cpu_permit);

    assert_eq!(
        db.get_federation_dkg_member_sequences()
            .await
            .expect("member sequences")
            .get(&peer_private.member_id),
        Some(&13)
    );
    assert_eq!(
        db.get_federation_dkg_member_keyset_shares()
            .await
            .expect("member keyset shares"),
        keyset_shares_before
    );
}

#[tokio::test]
async fn private_dkg_secret_share_route_rejects_wrong_receiver_before_sequence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let payload = dkg_secret_share_request(
        &public_config,
        peer_private.member_id,
        peer_private.member_id,
        21,
    );
    let request =
        signed_dkg_secret_share_request(&public_config, &peer_private, local_member, 1, payload);

    let response = post_dkg_secret_share(State(state.clone()), Json(request))
        .await
        .expect_err("wrong receiver must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
    let secret_shares = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("persisted secret-share contributions");
    assert!(secret_shares.is_empty());
}

#[tokio::test]
async fn private_dkg_secret_share_route_rejects_cross_ceremony_before_sequence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let commitment = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    let commitment_request =
        signed_dkg_commitment_request(&public_config, &peer_private, local_member, 1, commitment);
    let _ = post_dkg_commitment(State(state.clone()), Json(commitment_request))
        .await
        .expect("commitment accepted");

    let mut payload =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
    payload.ceremony_id = FederationDkgCeremonyId::from_bytes([56; 32]);
    let request =
        signed_dkg_secret_share_request(&public_config, &peer_private, local_member, 2, payload);
    let response = post_dkg_secret_share(State(state.clone()), Json(request))
        .await
        .expect_err("cross-ceremony secret share must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
    let secret_shares = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("persisted secret-share contributions");
    assert!(secret_shares.is_empty());
}

#[tokio::test]
async fn private_dkg_secret_share_route_rejects_conflict_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let commitment = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    let commitment_request =
        signed_dkg_commitment_request(&public_config, &peer_private, local_member, 1, commitment);
    let _ = post_dkg_commitment(State(state.clone()), Json(commitment_request))
        .await
        .expect("commitment accepted");
    let reveal = dkg_reveal_request(&public_config, peer_private.member_id, 21);
    let reveal_request =
        signed_dkg_reveal_request(&public_config, &peer_private, local_member, 2, reveal);
    let _ = post_dkg_reveal(State(state.clone()), Json(reveal_request))
        .await
        .expect("reveal accepted");
    let first_payload =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
    let conflicting_payload =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 31);
    let first_request = signed_dkg_secret_share_request(
        &public_config,
        &peer_private,
        local_member,
        3,
        first_payload.clone(),
    );
    let conflicting_request = signed_dkg_secret_share_request(
        &public_config,
        &peer_private,
        local_member,
        4,
        conflicting_payload,
    );

    let _ = post_dkg_secret_share(State(state.clone()), Json(first_request))
        .await
        .expect("first secret share accepted");
    let response = post_dkg_secret_share(State(state.clone()), Json(conflicting_request))
        .await
        .expect_err("conflicting secret share must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
    let secret_shares = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("persisted secret-share contributions");
    assert_eq!(secret_shares, vec![first_payload]);
}

#[tokio::test]
async fn private_dkg_secret_share_route_acks_before_driver_keyset_derivation() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    persist_complete_dkg_round(&state, &public_config, &local_private, &peer_private).await;

    let local_contribution =
        dkg_secret_share_request(&public_config, local_member, local_member, 11);
    state
        .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
        .await
        .expect("persist local secret-share contribution");
    assert!(
        state
            .mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await
            .expect("member keyset shares before final contribution")
            .is_empty(),
        "local keyset share should wait for every participant contribution"
    );

    let peer_contribution =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 21);
    let request = signed_dkg_secret_share_request(
        &public_config,
        &peer_private,
        local_member,
        3,
        peer_contribution.clone(),
    );

    let response = post_dkg_secret_share(State(state.clone()), Json(request))
        .await
        .expect("peer secret share accepted")
        .0;

    response
        .validate(
            public_config.federation_id,
            local_member,
            FederationRestMessageKind::DkgSecretShare,
            peer_private.member_id,
        )
        .expect("secret-share ack");
    let shares = state
        .mint
        .localstore()
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("member keyset shares");
    assert!(
        shares.is_empty(),
        "HTTP persistence must not derive the keyset"
    );
    let keyset_share = derive_and_persist_dkg_member_keyset_share(&state, &public_config).await;
    assert_eq!(keyset_share.member_id, local_member);
    assert_eq!(keyset_share.secret_shares.len(), 1);
    keyset_share
        .keyset
        .validate(public_config.threshold_params, &public_config.members)
        .expect("derived keyset validates");
    let contributions = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("stored secret-share contributions");
    assert_eq!(contributions, vec![local_contribution, peer_contribution]);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&local_member), Some(&3));
    assert_eq!(sequences.get(&peer_private.member_id), Some(&3));
}

#[tokio::test]
async fn private_dkg_secret_share_route_rejects_bad_share_before_persistence() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    persist_complete_dkg_round(&state, &public_config, &local_private, &peer_private).await;

    let bad_peer_contribution =
        dkg_secret_share_request(&public_config, peer_private.member_id, local_member, 31);
    let request = signed_dkg_secret_share_request(
        &public_config,
        &peer_private,
        local_member,
        3,
        bad_peer_contribution,
    );

    let response = post_dkg_secret_share(State(state.clone()), Json(request))
        .await
        .expect_err("bad secret share must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let shares = state
        .mint
        .localstore()
        .get_federation_dkg_member_keyset_shares()
        .await
        .expect("member keyset shares");
    assert!(shares.is_empty());
    let contributions = state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("stored secret-share contributions");
    assert!(contributions.is_empty());
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert_eq!(sequences.get(&local_member), Some(&2));
    assert_eq!(sequences.get(&peer_private.member_id), Some(&2));
}

#[tokio::test]
async fn federation_restart_activates_driver_derived_dkg_records() {
    let (trusted_public_config, local_private, peer_private) = transport_config();
    let mut setup_public_config = trusted_public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config.validate().expect("setup public config");

    let mut setup_local_private = local_private.clone();
    setup_local_private.keyset_secret_shares.clear();
    setup_local_private.setup_transcript_hash = None;
    setup_local_private
        .validate_against_public(&setup_public_config)
        .expect("setup local private config");
    let mut setup_peer_private = peer_private.clone();
    setup_peer_private.keyset_secret_shares.clear();
    setup_peer_private.setup_transcript_hash = None;

    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let state = federation_route_state_for_private_with_db(
        setup_public_config.clone(),
        setup_local_private.clone(),
        db.clone(),
        &seed,
    )
    .await;
    let local_member = setup_local_private.member_id;
    persist_complete_dkg_round(
        &state,
        &setup_public_config,
        &setup_local_private,
        &setup_peer_private,
    )
    .await;
    let local_contribution =
        dkg_secret_share_request(&setup_public_config, local_member, local_member, 11);
    state
        .persist_federation_dkg_secret_share_contribution(&local_contribution, local_member, 3)
        .await
        .expect("persist local secret-share contribution");
    let peer_contribution = dkg_secret_share_request(
        &setup_public_config,
        setup_peer_private.member_id,
        local_member,
        21,
    );
    let peer_request = signed_dkg_secret_share_request(
        &setup_public_config,
        &setup_peer_private,
        local_member,
        3,
        peer_contribution,
    );
    let _ = post_dkg_secret_share(State(state.clone()), Json(peer_request))
        .await
        .expect("peer secret-share accepted");

    let keyset_share =
        derive_and_persist_dkg_member_keyset_share(&state, &setup_public_config).await;
    let mut final_public_config = FederationConfig::new(
        setup_public_config.setup_authorization,
        setup_public_config.threshold_params,
        setup_public_config.members.clone(),
        vec![keyset_share.keyset.clone()],
    )
    .expect("final public config");
    final_public_config.payment_observation_policy =
        setup_public_config.payment_observation_policy.clone();
    let public_ceremonies = vec![keyset_share.public_ceremony.clone()];
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
    )
    .expect("transcript hash");
    let transcript_signatures = [&setup_local_private, &setup_peer_private]
        .into_iter()
        .map(|private_config| {
            FederationDkgTranscriptSignatureRequest::new(
                final_public_config.federation_id,
                private_config.member_id,
                transcript_hash,
                FederationDkgTranscriptSignature::sign(transcript_hash, private_config)
                    .expect("transcript signature"),
            )
        })
        .collect::<Vec<_>>();
    let transcript = FederationDkgTranscript::from_signatures(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
        transcript_signatures
            .iter()
            .map(|signature| (signature.signer, signature.signature))
            .collect(),
    )
    .expect("assembled transcript");
    final_public_config.setup_transcript = Some(transcript);
    final_public_config
        .validate()
        .expect("final public config with transcript");

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin transcript signatures");
    for signature in &transcript_signatures {
        tx.add_federation_dkg_transcript_signature(signature)
            .await
            .expect("persist transcript signature");
    }
    for private_config in [&setup_local_private, &setup_peer_private] {
        let activation = FederationDkgActivationRequest::sign(
            keyset_share.public_ceremony.ceremony_id(),
            &final_public_config,
            private_config,
        )
        .expect("activation confirmation");
        tx.add_federation_dkg_activation(&activation)
            .await
            .expect("persist activation confirmation");
    }
    tx.commit().await.expect("commit transcript signatures");

    let restarted = federation_route_state_for_private_with_db(
        setup_public_config,
        setup_local_private,
        db,
        &seed,
    )
    .await;
    let runtime_config = restarted
        .mint
        .federation_config()
        .expect("activated federation runtime config");
    assert_eq!(
        runtime_config.public_config().keysets,
        final_public_config.keysets
    );
    assert_eq!(
        runtime_config.public_config().setup_transcript,
        final_public_config.setup_transcript
    );
    assert_eq!(
        runtime_config
            .private_config()
            .keyset_secret_shares
            .get(&keyset_share.keyset.id),
        Some(&keyset_share.secret_shares)
    );
}

#[tokio::test]
async fn private_dkg_setup_route_rejects_sender_body_mismatch_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let mismatched_request = signed_dkg_reveal_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        dkg_reveal_request(&public_config, local_member, 11),
    );

    let response = post_dkg_reveal(State(state.clone()), Json(mismatched_request))
        .await
        .expect_err("mismatched setup request must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(sequences.is_empty());
}

#[tokio::test]
async fn private_dkg_reveal_route_rejects_missing_commitment_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let reveal_request = signed_dkg_reveal_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        dkg_reveal_request(&public_config, peer_private.member_id, 21),
    );

    let response = post_dkg_reveal(State(state.clone()), Json(reveal_request))
        .await
        .expect_err("reveal before commitment must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    let reveals = state
        .mint
        .localstore()
        .get_federation_dkg_reveals()
        .await
        .expect("persisted reveals");
    assert!(sequences.is_empty());
    assert!(reveals.is_empty());
}

#[tokio::test]
async fn private_dkg_commitment_route_rejects_conflict_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let first_payload = dkg_commitment_request(&public_config, peer_private.member_id, 21);
    let first_commitment = first_payload.commitment;
    let first_request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        first_payload,
    );
    let conflicting_request = signed_dkg_commitment_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        dkg_commitment_request(&public_config, peer_private.member_id, 31),
    );

    let _ = post_dkg_commitment(State(state.clone()), Json(first_request))
        .await
        .expect("first commitment accepted");
    let response = post_dkg_commitment(State(state.clone()), Json(conflicting_request))
        .await
        .expect_err("conflicting commitment must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    let commitments = state
        .mint
        .localstore()
        .get_federation_dkg_commitments()
        .await
        .expect("persisted commitments");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&1));
    assert_eq!(commitments, vec![first_commitment]);
}

#[tokio::test]
async fn private_dkg_reveal_route_rejects_conflict_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let commitment = dkg_commitment_request(&public_config, peer_private.member_id, 21).commitment;
    state
        .persist_federation_dkg_commitment(&commitment, peer_private.member_id, 1)
        .await
        .expect("persist DKG commitment");
    let first_payload = dkg_reveal_request(&public_config, peer_private.member_id, 21);
    let first_reveal = first_payload.reveal.clone();
    let first_request = signed_dkg_reveal_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        first_payload,
    );
    let conflicting_request = signed_dkg_reveal_request(
        &public_config,
        &peer_private,
        local_member,
        3,
        dkg_reveal_request(&public_config, peer_private.member_id, 31),
    );

    let _ = post_dkg_reveal(State(state.clone()), Json(first_request))
        .await
        .expect("first reveal accepted");
    let response = post_dkg_reveal(State(state.clone()), Json(conflicting_request))
        .await
        .expect_err("conflicting reveal must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    let reveals = state
        .mint
        .localstore()
        .get_federation_dkg_reveals()
        .await
        .expect("persisted reveals");
    assert_eq!(sequences.get(&peer_private.member_id), Some(&2));
    assert_eq!(reveals, vec![first_reveal]);
}
