use super::*;

#[tokio::test]
async fn catch_up_application_precedes_dependent_live_finality() {
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
    let quote_id = QuoteId::new();
    let prerequisite_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&state, &quote_id)),
            FederationConsensusItem::operation(paid_observation_envelope(&state, &quote_id)),
        ],
    )
    .expect("prerequisite finalized unit");
    let mut peer_consensus = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer consensus");
    peer_consensus
        .finalize_unit(prerequisite_unit)
        .expect("finalize prerequisite unit");
    let catch_up_request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response =
        aleph_bft_catch_up_response(&public_config, &peer_consensus, &catch_up_request);
    let (mint_envelope, output) =
        mint_operation_envelope(&state, &quote_id, b"serialized catch-up mint");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    let live_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(mint_envelope)],
    )
    .expect("dependent live unit");

    let application_lock = state.federation.operation_application_lock_for_test();
    let application_guard = application_lock.lock().await;
    let catch_up_state = state.clone();
    let mut catch_up = tokio::spawn(async move {
        catch_up_state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut catch_up)
            .await
            .is_err(),
        "catch-up should wait for the application boundary"
    );
    let live_state = state.clone();
    let mut live = tokio::spawn(async move {
        live_state
            .apply_federation_aleph_bft_finalized_unit(live_unit)
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut live)
            .await
            .is_err(),
        "live finality should queue behind catch-up application"
    );
    drop(application_guard);

    let caught_up = catch_up
        .await
        .expect("catch-up task")
        .expect("apply catch-up prerequisites");
    assert_eq!(caught_up.len(), 2);
    let responses = live
        .await
        .expect("live finality task")
        .expect("apply dependent live mint");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].operation_id, mint_operation_id);
    assert_eq!(
        responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("caught-up quote");
    assert_eq!(quote.amount_issued().value(), 1);
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("blind signature lookup")[0]
        .is_some());
}

#[tokio::test]
async fn lagging_member_refuses_public_federation_submit_until_healthy() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 1)
        .await;
    let envelope = test_operation_envelope_for_state(
        &state,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let blocked = state.submit_federation_envelope(envelope.clone()).await;

    assert!(matches!(
        blocked,
        Err(cdk::Error::Federation(FederationError::NodeLagging {
            peer,
            local_operation_count: 0,
            peer_operation_count: 1,
        })) if peer == peer_member.member_id
    ));
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Lagging { .. })
    ));
    let status_request = signed_status_request(&public_config, &peer_private, member_id(1), 1);
    let status = post_local_status(State(state.clone()), Json(status_request))
        .await
        .expect("lagging local status")
        .0;
    assert_eq!(status.sync_state, "lagging");
    assert_eq!(status.sync_peer, Some(peer_member.member_id));
    assert_eq!(status.sync_local_operation_count, Some(0));
    assert_eq!(status.sync_peer_operation_count, Some(1));
    assert_eq!(status.sync_operation_count, None);

    state.mark_federation_healthy().await;
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("healthy member can submit");
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test]
async fn lagging_member_refuses_public_restore_and_checkstate_reads_until_healthy() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 1)
        .await;
    let y = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"lagging member checkstate read",
    ));
    let restore_request = restore_request_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );

    let check_error = crate::router_handlers::post_check(
        AuthHeader::None,
        State(state.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect_err("lagging member must not serve authoritative check-state");
    let check_body = response_error_detail(check_error).await;
    assert!(
        check_body.contains("lagging behind peer"),
        "check-state should fail closed on federation lag, got: {check_body}"
    );

    let restore_error = crate::router_handlers::post_restore(
        AuthHeader::None,
        State(state.clone()),
        Json(restore_request.clone()),
    )
    .await
    .expect_err("lagging member must not serve authoritative restore");
    let restore_body = response_error_detail(restore_error).await;
    assert!(
        restore_body.contains("lagging behind peer"),
        "restore should fail closed on federation lag, got: {restore_body}"
    );

    state.mark_federation_healthy().await;
    let _ = crate::router_handlers::post_check(
        AuthHeader::None,
        State(state.clone()),
        Json(CheckStateRequest { ys: vec![y] }),
    )
    .await
    .expect("healthy member can serve check-state");
    let _ =
        crate::router_handlers::post_restore(AuthHeader::None, State(state), Json(restore_request))
            .await
            .expect("healthy member can serve restore");
}

#[tokio::test]
async fn peer_sync_round_catches_up_from_configured_peer() {
    let (state, public_config, local_private, peer_private, witness_private) =
        federation_route_state_three_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_quote_id = QuoteId::new();
    let peer_entry = mint_quote_envelope(&state, &peer_quote_id);
    let peer_state =
        federation_route_state_for_private(public_config.clone(), peer_private.clone()).await;
    peer_state
        .submit_federation_envelope(peer_entry)
        .await
        .expect("peer accepted operation");
    let (_, quorum) = recovery_checkpoint_quorum_for_state(
        &peer_state,
        &public_config,
        None,
        &[&peer_private, &witness_private],
    )
    .await;
    let mut tx = peer_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin peer checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify peer checkpoint quorum"),
    )
    .await
    .expect("promote peer recovery checkpoint");
    tx.commit().await.expect("commit peer recovery checkpoint");
    let mut tx = peer_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin peer backup transaction");
    tx.replace_federation_aleph_bft_backup_chunks(
        quorum.checkpoint.aleph_bft_session,
        &[vec![1; 16]],
    )
    .await
    .expect("persist peer backup");
    tx.set_federation_aleph_bft_active_session(quorum.checkpoint.aleph_bft_session)
        .await
        .expect("persist peer active session");
    tx.commit().await.expect("commit peer backup");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let transport = RouterFederationTransportRegistry::default();
    transport.insert(
        peer_member.federation_api_url.clone(),
        Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(peer_state),
    );
    let client = FederationRestPeerClient::new(transport, &public_config, local_private)
        .expect("checkpoint peer client");

    let discovery =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;
    assert_eq!(discovery, 1);
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        1,
        "checkpoint discovery must unblock catch-up in the same sync round"
    );
    let peer_status = client
        .sync_once_from_member(&state, peer_member)
        .await
        .expect("catch up from discovered checkpoint");
    assert_eq!(peer_status.operation_count, 1);
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        1
    );
    assert_eq!(
        state
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("local trusted checkpoint")
            .expect("promoted peer checkpoint")
            .checkpoint
            .id()
            .expect("local trusted checkpoint id"),
        quorum.checkpoint.id().expect("peer recovery checkpoint id")
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&peer_quote_id)
        .await
        .expect("caught-up quote lookup")
        .is_some());
}

#[tokio::test]
async fn equal_height_peer_sync_does_not_enter_catch_up() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        peer_member.member_id,
        0,
        None,
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    )
    .with_aleph_bft_next_unit_index_by_member(zero_aleph_bft_unit_cursor(&public_config));
    let transport = MockFederationTransport::default().with_status_response(status);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("peer client");

    let synced = client
        .sync_once_from_member(&state, peer_member)
        .await
        .expect("equal-height sync poll");

    assert_eq!(synced.operation_count, 0);
    assert!(transport.catch_up_requests().is_empty());
    assert_eq!(
        state
            .federation_sync_transition_stats()
            .await
            .expect("sync transition stats"),
        crate::FederationSyncTransitionStats::default()
    );
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test(start_paused = true)]
async fn peer_sync_round_backs_off_failed_peer_until_retry_delay() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let transport = MockFederationTransport::default();
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(8),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let mut backoff = FederationPeerReconnectBackoff::default();

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 1);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "retry before reconnect backoff expires must be skipped"
    );

    tokio::time::advance(Duration::from_secs(2) - Duration::from_millis(1)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "retry just before reconnect backoff expiry must still be skipped"
    );

    tokio::time::advance(Duration::from_millis(1)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 2);

    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    )
    .with_aleph_bft_next_unit_index_by_member(zero_aleph_bft_unit_cursor(&public_config));
    let _transport = transport.clone().with_status_response(status);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        2,
        "second failed attempt should double the reconnect backoff"
    );

    tokio::time::advance(Duration::from_secs(4)).await;
    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 1);
    assert_eq!(transport.status_requests().len(), 3);

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 1);
    assert_eq!(
        transport.status_requests().len(),
        4,
        "successful peer sync must clear reconnect backoff"
    );
}

#[tokio::test(start_paused = true)]
async fn peer_sync_round_skips_evidence_for_failed_or_backing_off_peer() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let entry = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    state
        .submit_federation_envelope(entry)
        .await
        .expect("local accepted operation");
    let transport = MockFederationTransport::default();
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            reconnect_initial_backoff: Duration::from_secs(2),
            reconnect_max_backoff: Duration::from_secs(8),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("client");
    let mut backoff = FederationPeerReconnectBackoff::default();

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(transport.status_requests().len(), 1);
    assert_eq!(
        transport.catch_up_requests().len(),
        0,
        "failed status sync must not immediately retry the peer through evidence collection"
    );

    let synced = run_federation_peer_sync_round_with_backoff(
        &state,
        &client,
        std::slice::from_ref(peer_member),
        &mut backoff,
    )
    .await;
    assert_eq!(synced, 0);
    assert_eq!(
        transport.status_requests().len(),
        1,
        "backing-off peer should not receive another status request"
    );
    assert_eq!(
        transport.catch_up_requests().len(),
        0,
        "backing-off peer should not receive evidence catch-up requests"
    );
}

#[tokio::test]
async fn peer_sync_round_does_not_clear_higher_known_lag_with_lower_peer() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    state
        .mark_federation_lagging(peer_member.member_id, 0, 2)
        .await;
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    )
    .with_aleph_bft_next_unit_index_by_member(zero_aleph_bft_unit_cursor(&public_config));
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let synced =
        run_federation_peer_sync_round(&state, &client, std::slice::from_ref(peer_member)).await;

    assert_eq!(synced, 1);
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Lagging {
            peer_operation_count: 2,
            ..
        })
    ));
}

#[tokio::test]
async fn single_equal_height_digest_liar_does_not_halt_four_honest_members() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let liar = private_configs.last().expect("liar config");
    let liar_member = public_config.member(liar.member_id).expect("liar member");

    for honest_private in private_configs.iter().take(4) {
        let state =
            federation_route_state_for_private(public_config.clone(), honest_private.clone()).await;
        let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
            &state,
            FederationPeerSyncConfig::default(),
            FederationAlephBftLiveSessionOptions::default(),
            RouterFederationTransportRegistry::default(),
        )
        .await
        .expect("start honest live session before digest mismatch")
        .expect("honest live session supervisor");
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);

        let liar_entry = mint_quote_envelope(&state, &QuoteId::new());
        let local_entry = mint_quote_envelope(&state, &QuoteId::new());
        let liar_consensus =
            finalized_aleph_bft_history(&public_config, liar.member_id, [liar_entry]);
        state
            .submit_federation_envelope(local_entry)
            .await
            .expect("honest member accepted local operation");
        let transport = MockFederationTransport::default().with_status_response(
            liar_consensus.peer_status_for(public_config.federation_id, liar.member_id),
        );
        let client =
            FederationRestPeerClient::new(transport, &public_config, honest_private.clone())
                .expect("honest peer client");

        let sync = client.sync_once_from_member(&state, liar_member).await;

        assert!(sync.is_err());
        assert!(
            !matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ),
            "one liar halted honest member {}",
            honest_private.member_id
        );
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);

        let neighboring_quote_id = QuoteId::new();
        state
            .submit_federation_envelope(mint_quote_envelope(&state, &neighboring_quote_id))
            .await
            .expect("honest neighboring operation progresses after liar rejection");
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&neighboring_quote_id)
            .await
            .expect("neighboring quote lookup")
            .is_some());

        super::super::halt_current_federation_aleph_bft_session(&state, "test cleanup").await;
        assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
    }
}

#[tokio::test]
async fn rest_peer_client_rejects_aleph_bft_status_without_finalized_cursor() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let mut status = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    status.aleph_bft_next_item_index = None;
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    let err = sync.expect_err("missing AlephBFT cursor should fail");
    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(FederationError::AlephBftFinalizedCursorMissing { member_id })
            if *member_id == peer_member.member_id
    ));
}

#[tokio::test]
async fn rest_peer_client_recovers_complete_aleph_bft_backup_pages_atomically() {
    let (state, public_config, local_private, peer_private, peer_member, peer_consensus) =
        backup_recovery_test_state().await;
    let mut peer_status = aleph_bft_peer_status_with_materialized_digest(
        &state,
        &public_config,
        peer_member.member_id,
        &peer_consensus,
    )
    .await;
    peer_status.aleph_bft_next_item_index = peer_status
        .aleph_bft_next_item_index
        .map(|next_item_index| next_item_index.saturating_add(3));
    peer_status.latest_audited_checkpoint_id = Some(
        cdk::federation::FederationCheckpointId::from_bytes([41; 32]),
    );
    peer_status.materialized_state_digest = None;
    let first_request = FederationAlephBftBackupRequest::new(0, 0);
    let first_response = FederationAlephBftBackupResponse::new(
        public_config.federation_id,
        peer_member.member_id,
        first_request,
        vec![vec![1, 2, 3]],
        false,
    );
    let second_request = FederationAlephBftBackupRequest::new(0, 1);
    let second_response = FederationAlephBftBackupResponse::new(
        public_config.federation_id,
        peer_member.member_id,
        second_request,
        vec![vec![4, 5]],
        true,
    );
    let db = state.mint.localstore();
    let frontier_before = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("checkpoint frontier before backup");
    let journal_before = db
        .get_federation_journal_entries()
        .await
        .expect("journal before backup");
    let trusted_before = db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint before backup");
    let materialized_before = state
        .mint
        .federation_materialized_state_snapshot()
        .await
        .expect("materialized state before backup")
        .digest()
        .expect("materialized digest before backup");
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_aleph_bft_backup_response(first_response, peer_private.clone())
        .with_aleph_bft_backup_response(second_response, peer_private);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("peer client");

    time::timeout(
        Duration::from_secs(5),
        client.sync_once_from_member(&state, &peer_member),
    )
    .await
    .expect("paged AlephBFT backup recovery completed")
    .expect("recover paged AlephBFT backup");

    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_backup_chunks(0)
            .await
            .expect("recovered backup chunks"),
        vec![vec![1, 2, 3], vec![4, 5]]
    );
    assert_eq!(
        db.get_federation_checkpoint_frontier()
            .await
            .expect("checkpoint frontier after backup"),
        frontier_before
    );
    assert_eq!(
        serde_json::to_value(
            db.get_federation_journal_entries()
                .await
                .expect("journal after backup")
        )
        .expect("journal JSON after backup"),
        serde_json::to_value(journal_before).expect("journal JSON before backup")
    );
    assert_eq!(
        db.get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted checkpoint after backup"),
        trusted_before
    );
    assert_eq!(
        state
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("materialized state after backup")
            .digest()
            .expect("materialized digest after backup"),
        materialized_before
    );
    let backup_offsets = transport
        .aleph_bft_backup_requests()
        .into_iter()
        .map(|request| request.payload.next_chunk)
        .collect::<Vec<_>>();
    assert_eq!(backup_offsets, vec![0, 1]);
    assert_eq!(
        transport.status_requests().len(),
        2,
        "backup recovery must refresh the replay frontier after the final page"
    );
}

#[tokio::test]
async fn rest_peer_client_rejects_malformed_backup_page_without_partial_persistence() {
    let (state, public_config, local_private, peer_private, peer_member, peer_consensus) =
        backup_recovery_test_state().await;
    let mut peer_status = aleph_bft_peer_status_with_materialized_digest(
        &state,
        &public_config,
        peer_member.member_id,
        &peer_consensus,
    )
    .await;
    peer_status.materialized_state_digest = None;
    let first_request = FederationAlephBftBackupRequest::new(0, 0);
    let first_response = FederationAlephBftBackupResponse::new(
        public_config.federation_id,
        peer_member.member_id,
        first_request,
        vec![vec![1, 2, 3]],
        false,
    );
    let malformed_response = FederationAlephBftBackupResponse::new(
        public_config.federation_id,
        peer_member.member_id,
        FederationAlephBftBackupRequest::new(0, 2),
        vec![vec![4, 5]],
        true,
    );
    let transport = MockFederationTransport::default()
        .with_status_response(peer_status)
        .with_aleph_bft_backup_response(first_response, peer_private.clone())
        .with_aleph_bft_backup_response(malformed_response, peer_private);
    let client = FederationRestPeerClient::new(transport, &public_config, local_private)
        .expect("peer client");

    let err = time::timeout(
        Duration::from_secs(5),
        client.sync_once_from_member(&state, &peer_member),
    )
    .await
    .expect("malformed AlephBFT backup rejection completed")
    .expect_err("out-of-order backup page must fail");

    assert!(err.to_string().contains("session or chunk cursor mismatch"));
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_chunks(0)
        .await
        .expect("backup chunks after malformed page")
        .is_empty());
}

#[tokio::test]
async fn rest_peer_client_uses_status_as_cheap_order_status() {
    let (state, public_config, local_private, _) =
        federation_route_state_with_local_private().await;
    let federation_id = public_config.federation_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let status = FederationPeerStatus::new_with_materialized_state_digest(
        federation_id,
        peer_member.member_id,
        0,
        None,
        cdk::federation::FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::from_bytes([9; 32]),
    )
    .with_aleph_bft_next_unit_index_by_member(zero_aleph_bft_unit_cursor(&public_config));
    let transport = MockFederationTransport::default().with_status_response(status);
    let client =
        FederationRestPeerClient::new(transport, &public_config, local_private).expect("client");

    let sync = client.sync_once_from_member(&state, peer_member).await;

    assert!(sync.is_ok());
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
}

#[tokio::test]
async fn catch_up_applies_mint_quote_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted quote");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_reports_conflicting_payment_observations() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let first_observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let conflicting_observation = mint_payment_observation_envelope_for_member(
        &state,
        &quote_id,
        2,
        2,
        "different-settlement",
    );
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    for envelope in [
        mint_quote_envelope(&state, &quote_id),
        first_observation,
        conflicting_observation,
    ] {
        finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted operation");
    }
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    assert_eq!(applied.len(), 3);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.payment_observation_reports.len(), 1);
    let report = &response.payment_observation_reports[0];
    assert_eq!(
        report.context,
        cdk::federation::FederationPaymentObservationContext::MintQuote
    );
    assert_eq!(report.quote, quote_id.to_string());
    assert_eq!(report.total_observations, 2);
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::Conflicting
    );
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
    assert!(!status_json.contains("different-settlement"));
}

#[tokio::test]
async fn catch_up_applies_melt_quote_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let invoice = create_fake_invoice(1000, "".to_string());
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: invoice.clone(),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let envelope = test_melt_quote_operation(&config, quote_id.clone(), request);
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt quote");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.id, quote_id);
    assert_eq!(
        quote.payment_method,
        PaymentMethod::Known(KnownMethod::Bolt11)
    );
    assert_eq!(quote.request.to_string(), invoice.to_string());
    assert_eq!(quote.state, MeltQuoteState::Unpaid);
    assert_eq!(quote.amount(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.fee_reserve(), Amount::new(1, CurrencyUnit::Sat));

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_swap_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let input = proof_for_amount(&state, 1, "catch-up");
    let input_y = input.y().expect("input y");
    let output = swap_output(&state, "catch-up");
    let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
    let envelope = config
        .swap_operation(request.clone())
        .expect("swap envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted swap");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&[input_y])
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let expected_response = config
        .swap_share_response(&persisted[0], &request)
        .expect("expected share response");
    assert_eq!(
        signatures,
        vec![Some(expected_response.signatures[0].clone())]
    );

    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_marks_conflicting_swap_rejected_and_continues_replay() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let first_input = proof_for_amount(&state, 1, "catch-up-conflict-first");
    let first_input_y = first_input.y().expect("first input y");
    let first_output = swap_output(&state, "catch-up-conflict-first");
    let first_request =
        cdk::nuts::SwapRequest::new(vec![first_input.clone()], vec![first_output.clone()]);
    let first_envelope = config
        .swap_operation(first_request.clone())
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "catch-up-conflict-loser");
    let conflicting_request =
        cdk::nuts::SwapRequest::new(vec![first_input], vec![conflicting_output.clone()]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");

    let later_input = proof_for_amount(&state, 1, "catch-up-conflict-later");
    let later_input_y = later_input.y().expect("later input y");
    let later_output = swap_output(&state, "catch-up-conflict-later");
    let later_request = cdk::nuts::SwapRequest::new(vec![later_input], vec![later_output.clone()]);
    let later_envelope = config
        .swap_operation(later_request.clone())
        .expect("later swap envelope");
    let later_operation_id = later_envelope.operation_id().expect("later operation id");

    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    for envelope in [first_envelope, conflicting_envelope, later_envelope] {
        finalize_test_operation(&mut peer_consensus, envelope)
            .expect("peer accepted swap operation");
    }
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");
    assert_eq!(catch_up_response.entries.len(), 3);

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("catch-up should continue past deterministic rejected swap");

    assert_eq!(applied.len(), 3);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_proofs_states(&[first_input_y, later_input_y])
            .await
            .expect("proof state lookup"),
        vec![Some(cdk::nuts::State::Spent), Some(cdk::nuts::State::Spent)]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let first_entry = persisted
        .iter()
        .find(|entry| entry.operation_id == first_operation_id)
        .expect("first accepted entry");
    let later_entry = persisted
        .iter()
        .find(|entry| entry.operation_id == later_operation_id)
        .expect("later accepted entry");
    let first_response = config
        .swap_share_response(first_entry, &first_request)
        .expect("first expected share response");
    let later_response = config
        .swap_share_response(later_entry, &later_request)
        .expect("later expected share response");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_output.blinded_secret,
                conflicting_output.blinded_secret,
                later_output.blinded_secret,
            ])
            .await
            .expect("signature lookup"),
        vec![
            Some(first_response.signatures[0].clone()),
            None,
            Some(later_response.signatures[0].clone()),
        ]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].operation_id, conflicting_operation_id);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status(), FederationOperationStatus::Applied);

    let runtime_consensus = local_consensus.lock().await;
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(first_operation_id)
            .expect("first runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(conflicting_operation_id)
            .expect("conflicting runtime entry")
            .status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(
        runtime_consensus
            .journal()
            .entry_by_operation_id(later_operation_id)
            .expect("later runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_applies_melt_operation_to_local_store() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = melt_quote(&state, 1000).await;
    let inputs = vec![
        proof_for_amount(&state, 1, "melt-catch-up-a"),
        proof_for_amount(&state, 1, "melt-catch-up-b"),
        proof_for_amount(&state, 1, "melt-catch-up-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "melt-catch-up-change");
    change_output.amount = Amount::ZERO;
    let request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let envelope = config.melt_operation(request).expect("melt envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    let payment_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some(String::new()),
    );
    let payment_operation_id = payment_envelope
        .operation_id()
        .expect("payment operation id");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut peer_consensus = local_consensus.lock().await.clone();
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt");
    finalize_test_operation(&mut peer_consensus, payment_envelope)
        .expect("peer accepted melt payment observation");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    let signature = signatures[0].clone().expect("change share persisted");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, change_output.keyset_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let runtime_consensus = local_consensus.lock().await;
    for operation_id in [operation_id, payment_operation_id] {
        assert_eq!(
            runtime_consensus
                .journal()
                .entry_by_operation_id(operation_id)
                .expect("runtime entry")
                .status(),
            FederationOperationStatus::Applied
        );
    }
}

#[tokio::test]
async fn catch_up_applies_paid_melt_payment_observation_to_pending_melt() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "paid-observation-a"),
        proof_for_amount(&state, 1, "paid-observation-b"),
        proof_for_amount(&state, 1, "paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("federation-paid-preimage".to_string()),
    );
    let paid_operation_id = paid_envelope.operation_id().expect("paid operation id");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut peer_consensus = local_consensus.lock().await.clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");
    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);
    assert_eq!(
        quote.payment_proof.as_deref(),
        Some("federation-paid-preimage")
    );

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    let signature = signatures[0].clone().expect("change share persisted");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, change_output.keyset_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let runtime = local_consensus.lock().await;
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(melt_operation_id)
            .expect("melt runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        runtime
            .journal()
            .entry_by_operation_id(paid_operation_id)
            .expect("paid runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_resolves_pending_melt_after_later_paid_observation() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "late-paid-observation-a"),
        proof_for_amount(&state, 1, "late-paid-observation-b"),
        proof_for_amount(&state, 1, "late-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "late-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("late-paid-observation-preimage".to_string()),
    );
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut peer_consensus = local_consensus.lock().await.clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    let pending_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("pending proof states");
    assert_eq!(
        pending_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("pending change signature lookup")[0]
        .is_none());

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("paid catch-up response");
    state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up");

    let paid_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote status");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote.change().expect("paid quote change");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);

    let spent_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("spent proof states");
    assert_eq!(
        spent_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "paid observation catch-up should clear pending melt saga metadata"
    );
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        3
    );
}

#[tokio::test]
async fn catch_up_rolls_back_pending_melt_after_later_failed_observation() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "late-failed-observation-a"),
        proof_for_amount(&state, 1, "late-failed-observation-b"),
        proof_for_amount(&state, 1, "late-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "late-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    let pending_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("pending proof states");
    assert_eq!(
        pending_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("failed catch-up response");
    state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up");

    let failed_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("failed quote status");
    assert_eq!(failed_quote.state(), MeltQuoteState::Unpaid);
    assert!(failed_quote.payment_proof().is_none());
    assert!(failed_quote.change().is_none());
    let released_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("released proof states");
    assert_eq!(released_states, vec![None, None, None]);
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup")[0]
        .is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "failed observation catch-up should clear pending melt saga metadata"
    );
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        3
    );
}

#[tokio::test]
async fn restart_replay_preserves_pending_melt_then_catches_up_paid_observation() {
    let (mut state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "restart-paid-observation-a"),
        proof_for_amount(&state, 1, "restart-paid-observation-b"),
        proof_for_amount(&state, 1, "restart-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "restart-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("restart-paid-observation-preimage".to_string()),
    );
    let mut peer_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status before replay");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before replay")
        .is_some());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
    let replayed_pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status after replay");
    assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
    assert!(replayed_pending_quote.change().is_none());

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let replayed_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus");
    let paid_response = checked_test_catch_up_response(
        &peer_consensus,
        &replayed_consensus.lock().await.catch_up_request(),
    )
    .expect("paid catch-up response after replay");
    state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up after replay");

    let paid_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote status after replay");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote.change().expect("paid quote change after replay");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after replay");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after paid catch-up")
            .is_none(),
        "paid observation after replay should clear pending melt saga metadata"
    );
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        3
    );
}

#[tokio::test]
async fn restart_replay_preserves_pending_melt_then_catches_up_failed_observation() {
    let (mut state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "restart-failed-observation-a"),
        proof_for_amount(&state, 1, "restart-failed-observation-b"),
        proof_for_amount(&state, 1, "restart-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "restart-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let pending_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("pending catch-up response");
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");

    let pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status before replay");
    assert_eq!(pending_quote.state(), MeltQuoteState::Pending);
    assert!(pending_quote.change().is_none());
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before replay")
        .is_some());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );
    let replayed_pending_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote status after replay");
    assert_eq!(replayed_pending_quote.state(), MeltQuoteState::Pending);
    assert!(replayed_pending_quote.change().is_none());

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let replayed_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus");
    let failed_response = checked_test_catch_up_response(
        &peer_consensus,
        &replayed_consensus.lock().await.catch_up_request(),
    )
    .expect("failed catch-up response after replay");
    state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up after replay");

    let rolled_back_quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("rolled back quote status after replay");
    assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
    assert!(rolled_back_quote.payment_proof().is_none());
    assert!(rolled_back_quote.change().is_none());
    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after rollback");
    assert_eq!(proof_states, vec![None, None, None]);
    assert!(state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup after replay")[0]
        .is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after failed catch-up")
            .is_none(),
        "failed observation after replay should clear pending melt saga metadata"
    );
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        3
    );
}

#[tokio::test]
async fn sqlite_restart_replays_pending_melt_then_catches_up_paid_observation() {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-restart");
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        db,
        &seed,
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-a"),
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-b"),
        proof_for_amount(&state, 1, "sqlite-restart-paid-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "sqlite-restart-paid-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let paid_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("sqlite-restart-paid-observation-preimage".to_string()),
    );
    let mut peer_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation.consensus.as_ref().expect("consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &local_consensus.lock().await.catch_up_request(),
        )
        .expect("pending catch-up response")
    };
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");
    assert_eq!(
        state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote before restart")
            .state(),
        MeltQuoteState::Pending
    );
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before restart")
        .is_some());
    drop(state);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );

    let restarted_pending_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote after sqlite restart");
    assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
    assert!(restarted_pending_quote.change().is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup after sqlite restart")
        .is_some());

    finalize_test_operation(&mut peer_consensus, paid_envelope)
        .expect("peer accepted paid observation");
    let paid_response = {
        let replayed_consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &replayed_consensus.lock().await.catch_up_request(),
        )
        .expect("paid catch-up response after sqlite restart")
    };
    restarted_state
        .apply_federation_catch_up_response(&paid_response)
        .await
        .expect("apply paid catch-up after sqlite restart");

    let paid_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("paid quote after sqlite restart");
    assert_eq!(paid_quote.state(), MeltQuoteState::Paid);
    let change = paid_quote
        .change()
        .expect("paid quote change after restart");
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].amount, Amount::from(1));
    assert_eq!(change[0].keyset_id, change_output.keyset_id);
    let proof_states = restarted_state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after sqlite restart");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert!(
        restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after paid catch-up")
            .is_none(),
        "paid observation after sqlite restart should clear pending melt saga metadata"
    );
    assert_eq!(
        restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        3
    );
}

#[tokio::test]
async fn sqlite_restart_replays_pending_melt_then_catches_up_failed_observation() {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-fed-pending-melt-failed-restart");
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        db,
        &seed,
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-a"),
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-b"),
        proof_for_amount(&state, 1, "sqlite-restart-failed-observation-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "sqlite-restart-failed-observation-change");
    change_output.amount = Amount::ZERO;
    let melt_request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let failed_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Failed, None);
    let mut peer_consensus = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");

    let pending_response = {
        let local_consensus = state.federation.consensus.as_ref().expect("consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &local_consensus.lock().await.catch_up_request(),
        )
        .expect("pending catch-up response")
    };
    state
        .apply_federation_catch_up_response(&pending_response)
        .await
        .expect("apply pending catch-up");
    assert_eq!(
        state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending quote before restart")
            .state(),
        MeltQuoteState::Pending
    );
    assert!(state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup before restart")
        .is_some());
    drop(state);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;
    assert_eq!(
        restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        2
    );

    let restarted_pending_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("pending quote after sqlite restart");
    assert_eq!(restarted_pending_quote.state(), MeltQuoteState::Pending);
    assert!(restarted_pending_quote.change().is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(&quote_id)
        .await
        .expect("saga lookup after sqlite restart")
        .is_some());

    finalize_test_operation(&mut peer_consensus, failed_envelope)
        .expect("peer accepted failed observation");
    let failed_response = {
        let replayed_consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("replayed consensus");
        checked_test_catch_up_response(
            &peer_consensus,
            &replayed_consensus.lock().await.catch_up_request(),
        )
        .expect("failed catch-up response after sqlite restart")
    };
    restarted_state
        .apply_federation_catch_up_response(&failed_response)
        .await
        .expect("apply failed catch-up after sqlite restart");

    let rolled_back_quote = restarted_state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .expect("rolled back quote after sqlite restart");
    assert_eq!(rolled_back_quote.state(), MeltQuoteState::Unpaid);
    assert!(rolled_back_quote.payment_proof().is_none());
    assert!(rolled_back_quote.change().is_none());
    let proof_states = restarted_state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup after sqlite restart");
    assert_eq!(proof_states, vec![None, None, None]);
    assert!(restarted_state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("failed change signature lookup after sqlite restart")[0]
        .is_none());
    assert!(
        restarted_state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup after failed catch-up")
            .is_none(),
        "failed observation after sqlite restart should clear pending melt saga metadata"
    );
    assert_eq!(
        restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("final consensus")
            .lock()
            .await
            .peer_status()
            .operation_count,
        3
    );
}

#[tokio::test]
async fn catch_up_applies_unpaid_melt_payment_observation_to_pending_melt() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let quote_id = melt_quote_with_description(
        &state,
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    )
    .await;
    let inputs = vec![
        proof_for_amount(&state, 1, "unpaid-observation-a"),
        proof_for_amount(&state, 1, "unpaid-observation-b"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let melt_request = cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, None);
    let melt_envelope = config.melt_operation(melt_request).expect("melt envelope");
    let unpaid_envelope =
        melt_payment_observation_envelope(&state, &quote_id, MeltQuoteState::Unpaid, None);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut peer_consensus = local_consensus.lock().await.clone();
    finalize_test_operation(&mut peer_consensus, melt_envelope)
        .expect("peer accepted pending melt");
    finalize_test_operation(&mut peer_consensus, unpaid_envelope)
        .expect("peer accepted unpaid observation");
    let catch_up_response = checked_test_catch_up_response(
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .expect("catch-up response");

    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Unpaid);
    assert!(quote.payment_proof.is_none());
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none(),
        "rollback should remove pending melt saga metadata"
    );

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![None, None]);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
}

#[tokio::test]
async fn offline_member_catches_up_multiple_operations_and_converges() {
    let (mut state, public_config, _, peer_private) =
        federation_route_state_with_local_private().await;
    let peer_state =
        federation_route_state_for_private(public_config.clone(), peer_private.clone()).await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);

    let mint_quote_id = QuoteId::new();
    let minted_output = mint_output(&state, &mint_quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![minted_output.clone()],
        signature: None,
    });
    let mint_envelope = config
        .mint_operation(mint_input.clone())
        .expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");

    let swap_input = proof_for_amount(&state, 1, "offline-catch-up-swap-input");
    let swap_input_y = swap_input.y().expect("swap input y");
    let swapped_output = swap_output(&state, "offline-catch-up-swap-output");
    let swap_request = cdk::nuts::SwapRequest::new(vec![swap_input], vec![swapped_output.clone()]);
    let swap_envelope = config
        .swap_operation(swap_request.clone())
        .expect("swap envelope");
    let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");

    let fake_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Pending,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(
        1000,
        serde_json::to_string(&fake_description).expect("fake invoice description"),
    );
    let melt_quote_envelope = test_melt_quote_operation(
        &config,
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: melt_invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_inputs = vec![
        proof_for_amount(&state, 1, "offline-catch-up-melt-a"),
        proof_for_amount(&state, 1, "offline-catch-up-melt-b"),
        proof_for_amount(&state, 1, "offline-catch-up-melt-c"),
    ];
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("melt input y"))
        .collect::<Vec<_>>();
    let mut melt_change_output = swap_output(&state, "offline-catch-up-melt-change");
    melt_change_output.amount = Amount::ZERO;
    let melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs,
        Some(vec![melt_change_output.clone()]),
    );
    let melt_envelope = config
        .melt_operation(melt_request.clone())
        .expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    let melt_payment_envelope = melt_payment_observation_envelope(
        &state,
        &melt_quote_id,
        MeltQuoteState::Paid,
        Some("offline-catch-up-preimage".to_string()),
    );

    for envelope in [
        mint_quote_envelope(&state, &mint_quote_id),
        paid_observation_envelope(&state, &mint_quote_id),
        mint_envelope.clone(),
        swap_envelope.clone(),
        melt_quote_envelope,
        melt_envelope.clone(),
    ] {
        peer_state
            .submit_federation_envelope_to_consensus(envelope.clone())
            .await
            .expect("peer accepted operation");
        finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted operation");
    }

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    assert_eq!(
        local_consensus.lock().await.peer_status().operation_count,
        0
    );
    let catch_up_response = catch_up_response_with_materialized_digest(
        &peer_state,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .await;

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply offline catch-up");

    assert_eq!(applied.len(), 6);
    assert_eq!(
        local_consensus
            .lock()
            .await
            .peer_status()
            .latest_order_digest,
        peer_consensus.journal().latest_order_digest()
    );

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("restart federation runtime from durable catch-up state");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
    activate_local_test_finality(&mut state);
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 6);
    assert!(persisted
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&mint_quote_id)
        .await
        .expect("mint quote lookup")
        .expect("mint quote persisted");
    assert_eq!(quote.state(), MintQuoteState::Issued);

    let mint_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[minted_output.blinded_secret])
        .await
        .expect("mint signature lookup");
    assert!(mint_signatures[0].is_some());

    let swap_states = state
        .mint
        .localstore()
        .get_proofs_states(&[swap_input_y])
        .await
        .expect("swap state lookup");
    assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
    let swap_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[swapped_output.blinded_secret])
        .await
        .expect("swap signature lookup");
    assert!(swap_signatures[0].is_some());

    let duplicate_mint = state
        .submit_federation_envelope(mint_envelope.clone())
        .await
        .expect("duplicate mint after catch-up");
    assert!(matches!(
        duplicate_mint.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let duplicate_mint_response = state
        .mint
        .persisted_mint_share_response(mint_operation_id, &mint_input)
        .await
        .expect("persisted duplicate mint share response");
    assert_eq!(duplicate_mint_response.signatures.len(), 1);

    let duplicate_swap = state
        .submit_federation_envelope(swap_envelope.clone())
        .await
        .expect("duplicate swap after catch-up");
    assert!(matches!(
        duplicate_swap.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let duplicate_swap_response = state
        .mint
        .persisted_swap_share_response(swap_operation_id, &swap_request)
        .await
        .expect("persisted duplicate swap share response");
    assert_eq!(duplicate_swap_response.signatures.len(), 1);

    let duplicate_pending_melt = state
        .submit_federation_envelope(melt_envelope.clone())
        .await
        .expect("duplicate pending melt after catch-up");
    assert!(matches!(
        duplicate_pending_melt.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let melt_quote = state
        .mint
        .localstore()
        .get_melt_quote(&melt_quote_id)
        .await
        .expect("melt quote lookup")
        .expect("melt quote persisted");
    assert_eq!(melt_quote.state, MeltQuoteState::Pending);
    assert!(melt_quote.payment_proof.is_none());
    let melt_states = state
        .mint
        .localstore()
        .get_proofs_states(&melt_input_ys)
        .await
        .expect("pending melt state lookup");
    assert_eq!(
        melt_states,
        vec![
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
            Some(cdk::nuts::State::Pending),
        ]
    );
    let pending_change_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[melt_change_output.blinded_secret])
        .await
        .expect("pending melt change signature lookup");
    assert!(pending_change_signatures[0].is_none());

    peer_state
        .submit_federation_envelope_to_consensus(melt_payment_envelope.clone())
        .await
        .expect("peer accepted delayed paid melt observation");
    finalize_test_operation(&mut peer_consensus, melt_payment_envelope)
        .expect("peer accepted delayed paid melt observation");
    let delayed_paid_response = catch_up_response_with_materialized_digest(
        &peer_state,
        &peer_consensus,
        &local_consensus.lock().await.catch_up_request(),
    )
    .await;
    let delayed_applied = state
        .apply_federation_catch_up_response(&delayed_paid_response)
        .await
        .expect("apply delayed paid melt catch-up");
    assert_eq!(delayed_applied.len(), 1);

    let melt_quote = state
        .mint
        .localstore()
        .get_melt_quote(&melt_quote_id)
        .await
        .expect("melt quote lookup after paid observation")
        .expect("melt quote persisted");
    assert_eq!(melt_quote.state, MeltQuoteState::Paid);
    assert_eq!(
        melt_quote.payment_proof.as_deref(),
        Some("offline-catch-up-preimage")
    );
    assert!(
        state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&melt_quote_id)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "paid observation replay should complete pending melt saga metadata"
    );

    let melt_states = state
        .mint
        .localstore()
        .get_proofs_states(&melt_input_ys)
        .await
        .expect("melt state lookup");
    assert_eq!(
        melt_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    let melt_change_signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[melt_change_output.blinded_secret])
        .await
        .expect("melt change signature lookup");
    assert!(melt_change_signatures[0].is_some());
    let duplicate_melt_response = state
        .mint
        .melt_federation(&melt_request, melt_operation_id)
        .await
        .expect("duplicate final melt response after delayed catch-up");
    assert_eq!(duplicate_melt_response.state(), MeltQuoteState::Paid);
    let duplicate_change = duplicate_melt_response
        .change()
        .expect("duplicate final melt change signatures");
    assert_eq!(duplicate_change.len(), 1);

    let local_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("local journal lookup after delayed catch-up");
    let peer_entries = peer_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal lookup after delayed catch-up");
    assert_eq!(local_entries.len(), 7);
    assert_eq!(peer_entries.len(), 7);
    let local_digest = materialized_state_digest(&state, &local_entries)
        .await
        .expect("local materialized digest");
    let peer_digest = materialized_state_digest(&peer_state, &peer_entries)
        .await
        .expect("peer materialized digest");
    assert_eq!(local_digest, peer_digest);

    let conflicting_output = swap_output(&state, "offline-catch-up-conflicting-mint");
    let conflicting_blinded_secret = conflicting_output.blinded_secret;
    let conflicting_mint = config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id,
            outputs: vec![conflicting_output],
            signature: None,
        }))
        .expect("conflicting mint envelope");
    let conflicting_operation_id = conflicting_mint
        .operation_id()
        .expect("conflicting mint operation id");
    let conflict = state.submit_federation_envelope(conflicting_mint).await;
    assert!(conflict.is_err());
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after conflicting mint");
    assert_eq!(entries.len(), 8);
    assert_eq!(entries[7].operation_id, conflicting_operation_id);
    assert_eq!(entries[7].status(), FederationOperationStatus::Rejected);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[conflicting_blinded_secret])
            .await
            .expect("conflicting mint signature lookup"),
        vec![None]
    );
}

#[tokio::test]
async fn diverged_member_rejects_peer_catch_up_digest_mismatch() {
    let (state, public_config, _) = federation_route_state().await;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let peer_entry = mint_quote_envelope(&state, &QuoteId::new());
    let local_entry = mint_quote_envelope(&state, &QuoteId::new());
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, peer_entry).expect("peer accepted operation");
    state
        .submit_federation_envelope(local_entry)
        .await
        .expect("local accepted divergent operation");

    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let request = local_consensus.lock().await.catch_up_request();
    assert_eq!(request.next_index, 1);
    assert!(matches!(
        checked_test_catch_up_response(&peer_consensus, &request),
        Err(FederationError::CatchUpDigestMismatch { .. })
    ));

    let unchecked_response = test_catch_up_response(&peer_consensus, &request);
    let apply = state
        .apply_federation_catch_up_response(&unchecked_response)
        .await;
    assert!(matches!(
        apply,
        Err(cdk::Error::Federation(
            FederationError::CatchUpDigestMismatch { .. }
        ))
    ));
}

#[tokio::test]
async fn federation_runtime_startup_persists_public_config_snapshot() {
    let (state, public_config, _) = federation_route_state().await;

    crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("runtime state");

    let snapshot = state
        .mint
        .localstore()
        .get_federation_config(public_config.federation_id)
        .await
        .expect("load federation config")
        .expect("stored federation config");
    assert_eq!(snapshot.config, public_config);
    assert_eq!(
        snapshot.config_digest,
        snapshot.config.config_digest().expect("config digest")
    );
}
