#[tokio::test]
async fn private_local_status_route_returns_consensus_and_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("submit mint quote");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin active backup status");
    tx.set_federation_aleph_bft_active_session(0)
        .await
        .expect("set active backup session");
    tx.replace_federation_aleph_bft_backup_chunks(0, &[vec![1, 2, 3]])
        .await
        .expect("persist active backup chunk");
    tx.commit().await.expect("commit active backup status");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state.clone()), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.version, FEDERATION_REST_API_VERSION);
    assert_eq!(response.peer_status.member_id, local_member);
    assert_eq!(response.peer_status.operation_count, 1);
    assert_eq!(response.sync_state, "healthy");
    assert!(response.sync_ready);
    assert_eq!(response.sync_message, "federation member is healthy");
    assert_eq!(response.checkpoint.frontier.journal_next_index, 1);
    assert_eq!(response.checkpoint.frontier.application_next_index, 1);
    assert!(response.checkpoint.trusted_checkpoint.is_none());
    assert_eq!(
        response
            .checkpoint
            .retained_recovery_floor
            .journal_from_index,
        0
    );
    assert_eq!(
        response
            .checkpoint
            .retained_recovery_floor
            .finalized_item_from_index,
        0
    );
    let active_backup = response
        .checkpoint
        .active_session_backup
        .expect("active-session backup status");
    assert_eq!(active_backup.session, 0);
    assert_eq!(active_backup.chunk_count, 1);
    assert_eq!(active_backup.total_bytes, 3);
    let aleph_bft = response
        .aleph_bft_status
        .as_ref()
        .expect("AlephBFT runtime status");
    assert_eq!(aleph_bft.retained_operation_from_index, 0);
    assert_eq!(aleph_bft.retained_operation_count, 1);
    assert_eq!(aleph_bft.retained_finalized_item_from_index, 0);
    assert_eq!(aleph_bft.retained_finalized_item_count, 1);
    assert!(aleph_bft.retained_finalized_canonical_bytes > 0);
    assert!(response.payment_observation_reports.is_empty());
    assert!(response.materialized_state_rows.is_empty());
    assert!(response.invalid_operation_alerts.is_empty());

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response with rows")
        .0;
    assert!(!response.materialized_state_rows.is_empty());
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(
        !status_json.contains("0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41")
    );
}

#[tokio::test]
async fn private_local_status_full_rows_replace_stale_cached_audit_digest() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("submit state captured by audit");
    let checkpoint = recovery_checkpoint_for_state(&state, &public_config, None).await;
    let checkpoint_id = checkpoint.id().expect("audited checkpoint id");
    let cached_audit_digest = checkpoint.materialized_state_digest;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin cached audit transaction");
    tx.add_federation_checkpoint_candidate(&checkpoint)
        .await
        .expect("persist audited checkpoint candidate");
    tx.record_federation_materialized_state_audit(&checkpoint)
        .await
        .expect("record cached materialized-state audit");
    tx.commit().await.expect("commit cached audit");

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("advance materialized state after audit");
    let current_digest = materialized_state_digest(&state, &[])
        .await
        .expect("current materialized-state digest");
    assert_ne!(current_digest, cached_audit_digest);

    let cached_request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let cached_response = post_local_status(State(state.clone()), Json(cached_request))
        .await
        .expect("cached local status response")
        .0;
    assert_eq!(
        cached_response.peer_status.materialized_state_digest,
        Some(cached_audit_digest)
    );
    assert_eq!(
        cached_response.peer_status.latest_audited_checkpoint_id,
        Some(checkpoint_id)
    );

    let exact_request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let exact_response = post_local_status(State(state), Json(exact_request))
        .await
        .expect("exact local status response")
        .0;
    assert_eq!(
        exact_response.peer_status.materialized_state_digest,
        Some(current_digest),
        "full-row status must replace the older cached audit digest"
    );
    assert_eq!(
        exact_response.peer_status.latest_audited_checkpoint_id, None,
        "the current snapshot digest must not be attributed to the cached checkpoint audit"
    );
    assert_eq!(
        exact_response
            .checkpoint
            .frontier
            .latest_audited_checkpoint_id,
        Some(checkpoint_id),
        "durable checkpoint status must continue to report the older cached audit"
    );
    assert!(!exact_response.materialized_state_rows.is_empty());
}

#[tokio::test]
async fn private_local_status_route_includes_unresolved_payment_observation_fingerprints() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            1,
            1,
            "row-fingerprint-payment-proof",
        ))
        .await
        .expect("submit unresolved payment observation");

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert!(response
        .materialized_state_rows
        .iter()
        .any(|row| row.table == "payment_observations"));
    assert!(response
        .payment_observation_reports
        .iter()
        .any(|report| report.quote == quote_id.to_string()
            && report.status
                == cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum));
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("row-fingerprint-payment-proof"));
}

#[tokio::test]
async fn private_local_status_route_includes_completed_operation_fingerprints() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let local_member = config.member_id();
    let quote_id = QuoteId::new();
    let output = mint_output(&state, &quote_id);
    let mint_envelope = config
        .mint_operation(MintInput::Single(MintRequest {
            quote: quote_id.clone(),
            outputs: vec![output.clone()],
            signature: None,
        }))
        .expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit paid observation");
    state
        .submit_federation_envelope(mint_envelope)
        .await
        .expect("submit mint issuance");

    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert!(response
        .materialized_state_rows
        .iter()
        .any(|row| row.table == "completed_operations"));
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains(&mint_operation_id.to_string()));
    assert!(
        !status_json.contains(&federation_completed_operation_uuid(mint_operation_id).to_string())
    );
    assert!(!status_json.contains(&output.blinded_secret.to_hex()));
}

#[tokio::test]
async fn private_local_status_route_reports_invalid_operation_alerts() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    for _ in 0..crate::FEDERATION_INVALID_OPERATION_ALERT_THRESHOLD {
        state
            .record_federation_invalid_operation_alert(
                crate::FederationInvalidOperationSource::auth_token("clear", b"secret-token"),
            )
            .await;
    }

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.invalid_operation_alerts.len(), 1);
    assert_eq!(response.invalid_operation_alerts[0].source, "auth_token");
    assert_eq!(response.invalid_operation_alerts[0].alerting_buckets, 1);
    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("secret-token"));
}

#[tokio::test]
async fn private_local_status_route_reports_payment_observation_conflicts() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first payment observation");
    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state,
            &quote_id,
            2,
            2,
            "different-settlement",
        ))
        .await
        .expect("submit conflicting second payment observation");
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
    assert_eq!(report.required, 1);
    assert_eq!(report.total_observations, 2);
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::Conflicting
    );
    assert_eq!(report.groups.len(), 2);
    assert_eq!(report.groups[0].state, "PAID");
    assert_eq!(report.groups[0].observers, vec![member_id(1)]);
    assert!(report.groups[0].quorum_reached);
    assert_eq!(report.groups[0].conflicting_observers, vec![member_id(2)]);
    assert_eq!(
        report.conflicting_observers,
        vec![member_id(1), member_id(2)]
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
async fn private_local_status_route_reports_stale_payment_observations() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit old mint quote");

    state
        .submit_federation_envelope(mint_payment_observation_envelope_for_member(
            &state, &quote_id, 1, 1, "settled",
        ))
        .await
        .expect("submit first payment observation");

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.payment_observation_reports.len(), 1);
    let report = &response.payment_observation_reports[0];
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum
    );
    assert_eq!(report.required, 2);
    assert_eq!(report.total_observations, 1);
    assert!(report.stale);
    assert!(report.quote_age_secs.expect("quote age") >= 1);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );
    assert!(!report.groups[0].quorum_reached);

    let status_json = serde_json::to_string(&response).expect("status json");
    assert!(!status_json.contains("settled"));
}

#[tokio::test]
async fn private_status_routes_read_persisted_journal_after_runtime_reset() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let envelope = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit restore");
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);

    reset_federation_runtime_journal(&mut state);
    assert_eq!(consensus_operation_count(&state).await, 0);

    let status_request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let status = post_status(State(state.clone()), Json(status_request))
        .await
        .expect("status response")
        .0;
    assert_eq!(status.operation_count, 1);
    assert_eq!(status.latest_index, Some(0));
    assert_eq!(status.latest_order_digest, entries[0].order_digest);
    assert!(
        status.materialized_state_digest.is_none(),
        "ordinary peer status must not perform a full materialized-state audit"
    );

    let local_status_request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let local_status = post_local_status(State(state), Json(local_status_request))
        .await
        .expect("local status response")
        .0;
    assert_eq!(local_status.peer_status.operation_count, 1);
    assert_eq!(local_status.peer_status.latest_index, Some(0));
    assert_eq!(
        local_status.peer_status.latest_order_digest,
        entries[0].order_digest
    );
    assert!(local_status.peer_status.materialized_state_digest.is_some());
    assert!(!local_status.materialized_state_rows.is_empty());
}

#[test]
fn aleph_bft_inbound_network_queue_rejects_flood_beyond_bounded_budget() {
    crate::FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_OVERRIDE.store(
        FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_TEST,
        std::sync::atomic::Ordering::Relaxed,
    );
    let _guard = AlephBftChannelCapacityOverrideGuard;
    let (_network, handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let inbound = CdkAlephBftNetworkInboundHandle::<u64>::new(0, &handle);
    let request =
        |sequence: u64| FederationAlephBftNetworkMessageRequest::new(0, sequence.encode());

    // An authenticated member flooding the private REST admission path must
    // be rejected with bounded backpressure instead of growing the inbound
    // queue without limit.
    let mut rejection = None;
    for sequence in 0..=FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_TEST {
        match inbound.queue_request(&request(sequence)) {
            Ok(_) => {}
            Err(err) => {
                rejection = Some(err);
                break;
            }
        }
    }
    let err = rejection.expect("flood beyond the bounded budget must be rejected");
    assert!(
        err.to_string().contains("backpressure"),
        "rejection must name bounded backpressure: {err}"
    );
}

struct AlephBftChannelCapacityOverrideGuard;

impl Drop for AlephBftChannelCapacityOverrideGuard {
    fn drop(&mut self) {
        crate::FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_OVERRIDE
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

#[tokio::test]
async fn aleph_bft_finalized_channel_backpressure_never_drops_and_overflow_halts_bounded() {
    let _budget_guard = ApplyRetryBudgetOverrideGuard::set(60_000);
    let session = start_two_member_aleph_bft_live_session(
        fast_aleph_bft_live_session_options(
            CdkAlephBftNetworkPumpConfig {
                session_id: 0,
                max_messages_per_tick: 128,
                max_peer_deliveries_per_message: 128,
            },
            500,
        )
        .with_aleph_bft_channel_capacity(32),
    )
    .await
    .expect("start two-member live session");

    // Member 1's finalized-unit application stays contended: its bounded
    // channel must hold the finalized work without loss while apply is stuck.
    session
        .first_state
        .federation
        .permanent_federation_apply_contention_for_test();
    let mut operation_ids = Vec::new();
    for _ in 0..24 {
        let envelope = mint_quote_envelope(&session.second_state, &QuoteId::new());
        let operation_id = envelope.operation_id().expect("operation id");
        session
            .second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect("submit operation");
        operation_ids.push(operation_id);
    }
    for operation_id in &operation_ids {
        assert_eq!(
            wait_for_terminal_operation(&session.second_state, *operation_id).await,
            FederationOperationStatus::Applied
        );
    }

    // With the backlog below the bounded budget, no finalized unit is
    // dropped: after the contention clears, every unit applies in order.
    session
        .first_state
        .federation
        .components
        .faults
        .permanent_federation_apply_contention
        .store(false, Ordering::Release);
    for operation_id in &operation_ids {
        assert_eq!(
            wait_for_terminal_operation(&session.first_state, *operation_id).await,
            FederationOperationStatus::Applied,
            "no finalized unit may be dropped while apply is behind"
        );
    }

    // Exceed the bounded budget: the session must tear down with a bounded
    // transition instead of growing memory without limit. The supervisor
    // rotates the session from durable history, so no finalized work is
    // lost.
    session
        .first_state
        .federation
        .permanent_federation_apply_contention_for_test();
    let overflow_state = session.second_state.clone();
    let mut overflow_operation_ids = Vec::new();
    let overflow_submissions = tokio::spawn(async move {
        for _ in 0..48 {
            let envelope = mint_quote_envelope(&overflow_state, &QuoteId::new());
            let operation_id = envelope.operation_id().expect("operation id");
            submit_live_operation_allowing_pending_timeout(&overflow_state, envelope).await;
            overflow_operation_ids.push(operation_id);
        }
    });
    time::timeout(Duration::from_secs(30), async {
        loop {
            if session._first_handle.state() == AlephBftSessionSupervisorState::Stopped {
                break;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("overflow must stop the session with a bounded transition");
    assert!(
        session
            .second_state
            .federation_aleph_bft_live_session_active()
            .await
    );

    // The rotated session replays from durable history: after the contention
    // clears, the backlogged units apply, proving nothing was dropped.
    session
        .first_state
        .federation
        .components
        .faults
        .permanent_federation_apply_contention
        .store(false, Ordering::Release);
    let mut applied_overflow = 0usize;
    time::timeout(Duration::from_secs(30), async {
        loop {
            let applied = session
                .first_state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal lookup after rotation")
                .len();
            if applied >= 24 {
                applied_overflow = applied;
                break;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("rotated session must replay the backlog without loss");
    assert!(
        applied_overflow >= 24,
        "rotated session must apply the backlogged finalized units"
    );
    overflow_submissions.abort();
    let _ = overflow_submissions.await;
}

#[tokio::test]
async fn private_status_route_records_sequence_without_persisting_liveness_watermark() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = signed_status_request(&public_config, &peer_private, local_member, 1);

    let status = post_status(State(state.clone()), Json(request.clone()))
        .await
        .expect("status response")
        .0;
    assert_eq!(status.member_id, local_member);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences");
    assert!(
        sequences.is_empty(),
        "status liveness requests should not persist sequence watermarks"
    );

    let response = post_status(State(state), Json(request))
        .await
        .expect_err("same status sequence should be rejected in memory");
    let detail = response_error_detail(response).await;
    assert!(
        detail.contains("stale sequence"),
        "unexpected detail: {detail}"
    );
}

#[tokio::test]
async fn private_status_route_rejects_inbound_overflow_before_sequence_persistence() {
    let (mut state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state.federation.private_rest_admission = Some(Arc::new(tokio::sync::Semaphore::new(1)));
    let held_permit = state
        .federation
        .private_rest_admission
        .as_ref()
        .expect("admission")
        .clone()
        .try_acquire_owned()
        .expect("held inbound permit");
    let request = signed_status_request(&public_config, &peer_private, local_member, 1);

    let response = post_status(State(state.clone()), Json(request.clone()))
        .await
        .expect_err("inbound overflow should fail");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "overflow must not persist the sender sequence"
    );

    drop(held_permit);
    let status = post_status(State(state), Json(request))
        .await
        .expect("status response after capacity is available")
        .0;
    assert_eq!(status.member_id, local_member);
}

#[tokio::test]
async fn private_local_status_route_reports_lagging_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state.mark_federation_lagging(peer_member, 0, 3).await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.peer_status.operation_count, 0);
    assert_eq!(response.sync_state, "lagging");
    assert!(!response.sync_ready);
    assert!(response
        .sync_message
        .contains("federation member is lagging behind peer"));
}

#[tokio::test]
async fn private_local_status_route_reports_catching_up_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let peer_member = peer_private.member_id;
    state.mark_federation_catching_up(peer_member).await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.sync_state, "catching_up");
    assert!(!response.sync_ready);
    assert_eq!(response.sync_peer, Some(peer_member));
    assert!(response.sync_message.contains("catching up from peer"));
}

#[tokio::test]
async fn replay_readiness_does_not_override_a_newer_catching_up_state() {
    let (state, _, peer_private) = federation_route_state().await;
    state
        .mark_federation_catching_up(peer_private.member_id)
        .await;

    state.federation.mark_healthy_after_replay().await;

    assert_eq!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::CatchingUp {
            peer: peer_private.member_id,
        })
    );
}

#[tokio::test]
async fn catching_up_member_blocks_wallet_observer_and_broadcaster_work() {
    let (state, _, peer_private) = federation_route_state().await;
    state
        .mark_federation_catching_up(peer_private.member_id)
        .await;

    assert!(super::payment_observation::FederationPaymentObserver
        .run_round(&state)
        .await
        .is_err());
    assert!(
        super::broadcast::FederationTransactionBroadcaster::default()
            .run_round(&state)
            .await
            .is_err()
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries")
        .is_empty());
}

#[tokio::test]
async fn private_local_status_route_reports_redacted_halted_sync_state() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    state
        .mark_federation_halted("digest mismatch token=operator-secret")
        .await;

    let request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let response = post_local_status(State(state), Json(request))
        .await
        .expect("local status response")
        .0;

    assert_eq!(response.sync_state, "halted");
    assert!(!response.sync_ready);
    assert_eq!(response.sync_peer, None);
    assert!(response.sync_message.contains("digest mismatch"));
    assert!(!response.sync_message.contains("operator-secret"));
}

#[tokio::test]
async fn startup_discards_transient_federation_sync_status() {
    let (state, _, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    state.mark_federation_lagging(peer_member, 2, 5).await;

    let mut restarted = state.clone();
    restarted
        .federation
        .set_sync_handle(Some(Arc::new(tokio::sync::Mutex::new(
            crate::FederationSyncState::for_aleph_bft(),
        ))));
    restarted.restore_federation_startup_readiness().await;

    assert_eq!(
        restarted.federation_sync_status().await,
        Some(crate::FederationSyncStatus::AwaitingQuorum { operation_count: 0 })
    );
}

#[tokio::test]
async fn private_journal_route_pages_to_checkpoint_without_range_signing() {
    let (state, public_config, local_private, peer_private) =
        federation_route_state_with_local_private().await;
    let local_member = local_private.member_id;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("first checkpoint operation");
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("second checkpoint operation");
    let (checkpoint, _quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("checkpoint id");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin checkpoint candidate transaction");
    tx.add_federation_checkpoint_candidate(&checkpoint)
        .await
        .expect("persist checkpoint candidate");
    tx.commit().await.expect("commit checkpoint candidate");
    assert!(state
        .mint
        .localstore()
        .get_federation_checkpoint_quorum(checkpoint_id)
        .await
        .expect("checkpoint quorum lookup")
        .is_none());

    let first_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationJournalCatchUpRequest::new(0)
            .with_target_checkpoint(checkpoint_id)
            .with_limit(1)
            .with_aleph_bft_next_item_index(0),
    );
    let first = post_journal(State(state.clone()), Json(first_request))
        .await
        .expect("first checkpoint page")
        .0;
    first
        .validate_checkpoint_binding(&checkpoint)
        .expect("first checkpoint page binding");
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.aleph_bft_finalized_items.len(), 1);
    assert!(!first.complete);
    assert!(!first.aleph_bft_finalized_items_complete);

    let second_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationJournalCatchUpRequest::new(1)
            .with_target_checkpoint(checkpoint_id)
            .with_limit(1)
            .with_known_order_digest(first.entries[0].order_digest)
            .with_aleph_bft_next_item_index(1),
    );
    let second = post_journal(State(state), Json(second_request))
        .await
        .expect("second checkpoint page")
        .0;
    second
        .validate_checkpoint_binding(&checkpoint)
        .expect("second checkpoint page binding");
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.aleph_bft_finalized_items.len(), 1);
    assert!(second.complete);
    assert!(second.aleph_bft_finalized_items_complete);
}

#[tokio::test]
async fn private_journal_route_returns_whole_aleph_bft_finalized_item_units() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let local_member = local_private.member_id;
    let first_quote_id = QuoteId::new();
    let second_quote_id = QuoteId::new();
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote_id)),
            FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote_id)),
        ],
    )
    .expect("two-item AlephBFT unit");
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply AlephBFT unit");

    let small_limit_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationJournalCatchUpRequest::new(0)
            .with_limit(1)
            .with_aleph_bft_next_item_index(0),
    );
    let small_limit_response = post_journal(State(state.clone()), Json(small_limit_request))
        .await
        .expect("small-limit journal response")
        .0;
    assert_eq!(small_limit_response.entries.len(), 2);
    assert_eq!(small_limit_response.aleph_bft_finalized_items.len(), 2);
    assert!(small_limit_response.complete);
    assert!(small_limit_response.aleph_bft_finalized_items_complete);
    assert!(small_limit_response
        .aleph_bft_finalized_items
        .iter()
        .all(|item| item.unit_index == 0));

    let complete_request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_member,
        2,
        FederationJournalCatchUpRequest::new(0)
            .with_limit(2)
            .with_aleph_bft_next_item_index(0),
    );
    let complete_response = post_journal(State(state), Json(complete_request))
        .await
        .expect("complete journal response")
        .0;
    assert_eq!(complete_response.entries.len(), 2);
    assert_eq!(complete_response.aleph_bft_finalized_items.len(), 2);
    assert!(complete_response.aleph_bft_finalized_items_complete);
    assert!(complete_response
        .aleph_bft_finalized_items
        .iter()
        .all(|item| item.unit_index == 0));
}

#[tokio::test]
async fn private_journal_route_aligns_claims_with_duplicate_finalized_items() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(envelope.clone()),
            FederationConsensusItem::operation(envelope),
        ],
    )
    .expect("duplicate-operation unit");
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply duplicate-operation unit");

    let request = signed_catch_up_request(
        &public_config,
        &peer_private,
        local_private.member_id,
        1,
        FederationJournalCatchUpRequest::new(0)
            .with_limit(1)
            .with_aleph_bft_next_item_index(0),
    );
    let response = post_journal(State(state), Json(request))
        .await
        .expect("duplicate-operation journal page")
        .0;
    assert_eq!(response.entries.len(), 1);
    assert_eq!(response.aleph_bft_finalized_items.len(), 2);
    assert_eq!(
        response.aleph_bft_finalized_items[0].status,
        FederationAlephBftFinalizedItemStatus::Accepted
    );
    assert_eq!(
        response.aleph_bft_finalized_items[1].status,
        FederationAlephBftFinalizedItemStatus::DuplicateOperation
    );
    assert!(response.complete);
    assert!(response.aleph_bft_finalized_items_complete);
}

#[tokio::test]
async fn finalized_item_catch_up_pages_mixed_suffix_without_count_mismatch() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    // The reported reproduction plus every mixed composition: unit 0 carries
    // an accepted operation, its duplicate, a rejected recovery barrier, and
    // an accepted non-operation item; unit 1 carries a second accepted
    // operation; unit 2 carries only another duplicate. Six finalized items
    // reconstruct two journal entries, so any one-to-one page alignment
    // assumption produces the reported count mismatch.
    let accepted_a = mint_quote_envelope(&state, &QuoteId::new());
    let invalid_barrier =
        FederationRecoveryBarrier::new(public_config.federation_id, 0, None, u64::MAX)
            .expect("out-of-position recovery barrier");
    let unit_zero = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(accepted_a.clone()),
            FederationConsensusItem::operation(accepted_a.clone()),
            FederationConsensusItem::recovery_barrier(invalid_barrier),
            FederationConsensusItem::Default(FederationConsensusDefaultItem::new(
                "module-fact",
                serde_json::json!({"fact": 1}),
            )),
        ],
    )
    .expect("mixed unit");
    let accepted_b = mint_quote_envelope(&state, &QuoteId::new());
    let unit_one = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(accepted_b)],
    )
    .expect("second accepted unit");
    let unit_two = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        2,
        vec![FederationConsensusItem::operation(accepted_a)],
    )
    .expect("duplicate-only unit");
    for unit in [unit_zero, unit_one, unit_two] {
        state
            .apply_federation_aleph_bft_finalized_unit(unit)
            .await
            .expect("apply finalized unit");
    }

    // Walk the real route and the real client check page by page with a
    // limit that intersects the four-item unit. Every page must pair its
    // journal claims with the exact finalized prefix that replays to them.
    let mut client_items = Vec::new();
    let mut client_entry_count = 0usize;
    let mut next_index = 0u64;
    let mut next_item_index = 0u64;
    let mut first_page = None;
    let expected_item_counts = [4usize, 1, 1];
    let expected_entry_counts = [1usize, 1, 0];
    let mut final_response = None;
    for (sequence, (expected_items, expected_entries)) in expected_item_counts
        .iter()
        .zip(expected_entry_counts.iter())
        .enumerate()
    {
        let request = signed_catch_up_request(
            &public_config,
            &peer_private,
            local_private.member_id,
            u64::try_from(sequence).expect("sequence") + 1,
            FederationJournalCatchUpRequest::new(next_index)
                .with_limit(1)
                .with_aleph_bft_next_item_index(next_item_index),
        );
        let response = post_journal(State(state.clone()), Json(request))
            .await
            .expect("mixed-suffix catch-up page")
            .0;
        assert_eq!(
            response.aleph_bft_finalized_items.len(),
            *expected_items,
            "page {sequence} must carry complete units only"
        );
        assert_eq!(
            response.entries.len(),
            *expected_entries,
            "page {sequence} journal claims must match the finalized replay prefix"
        );

        client_items.extend(response.aleph_bft_finalized_items.iter().cloned());
        let consensus = FederationConsensusRuntime::recover_finalized_items_for_test(
            &public_config,
            local_private.member_id,
            0,
            client_items.clone(),
        )
        .expect("reconstruct finalized history");
        let recovered_entries = consensus.journal().entries();
        crate::ensure_catch_up_entries_match_recovered_journal(
            &recovered_entries[client_entry_count..],
            Some(&response),
        )
        .expect("page must match the recovered journal exactly");
        client_entry_count = recovered_entries.len();
        next_index = response
            .from_index
            .checked_add(u64::try_from(response.entries.len()).expect("page length"))
            .expect("journal cursor advance");
        next_item_index = next_item_index
            .checked_add(u64::try_from(response.aleph_bft_finalized_items.len()).expect("items"))
            .expect("finalized cursor advance");
        if sequence == 0 {
            first_page = Some(response.clone());
        }
        final_response = Some((response, consensus));
    }

    let (response, consensus) = final_response.expect("duplicate-only final page");
    assert!(
        response.complete,
        "duplicate-only page must complete the walk"
    );
    assert!(response.aleph_bft_finalized_items_complete);
    assert_eq!(client_entry_count, 2);
    assert_eq!(
        response.latest_order_digest,
        consensus.journal().latest_order_digest(),
        "complete walk must converge on the server journal digest"
    );
    let statuses: Vec<_> = client_items.iter().map(|item| item.status).collect();
    assert_eq!(
        statuses,
        vec![
            FederationAlephBftFinalizedItemStatus::Accepted,
            FederationAlephBftFinalizedItemStatus::DuplicateOperation,
            FederationAlephBftFinalizedItemStatus::RejectedInvalidRecoveryBarrier,
            FederationAlephBftFinalizedItemStatus::Accepted,
            FederationAlephBftFinalizedItemStatus::Accepted,
            FederationAlephBftFinalizedItemStatus::DuplicateOperation,
        ]
    );

    // The exact comparison must still reject a genuinely mismatched pair:
    // replaying the full six-item suffix recovers two journal entries, which
    // must not be accepted against the first page's single-entry claims.
    let full_consensus = FederationConsensusRuntime::recover_finalized_items_for_test(
        &public_config,
        local_private.member_id,
        0,
        client_items,
    )
    .expect("reconstruct full finalized history");
    let mismatch = crate::ensure_catch_up_entries_match_recovered_journal(
        full_consensus.journal().entries(),
        Some(&first_page.expect("first page retained")),
    )
    .expect_err("mismatched page must be rejected");
    assert!(
        mismatch
            .to_string()
            .contains("recovered 2 journal entries but response carried 1"),
        "rejection must name the exact count mismatch: {mismatch}"
    );
}

#[tokio::test]
async fn private_backup_route_pages_aleph_bft_backup_with_independent_bounds() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let first = vec![1; 700 * 1024];
    let second = vec![2; 700 * 1024];
    let third = vec![3; 700 * 1024];
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin backup seed");
    for chunk in [&first, &second, &third] {
        tx.append_federation_aleph_bft_backup_bytes(4, chunk)
            .await
            .expect("append backup chunk");
    }
    tx.commit().await.expect("commit backup seed");

    for (sequence, next_chunk, expected_byte, complete) in
        [(1, 0, 1, false), (2, 1, 2, false), (3, 2, 3, true)]
    {
        let request = FederationSignedRequest::new_signed(
            public_config.federation_id,
            local_member,
            sequence,
            FederationRestMessageKind::AlephBftBackup,
            FederationAlephBftBackupRequest::new(4, next_chunk),
            &peer_private,
        )
        .expect("signed backup request");
        let response = post_aleph_bft_backup(State(state.clone()), Json(request))
            .await
            .expect("backup page response")
            .0
            .payload;
        let backup = response;
        assert_eq!(backup.session, 4);
        assert_eq!(backup.from_chunk, next_chunk);
        assert_eq!(backup.chunks.len(), 1);
        assert_eq!(backup.chunks[0].len(), 700 * 1024);
        assert!(backup.chunks[0].iter().all(|byte| *byte == expected_byte));
        assert_eq!(backup.complete, complete);
    }
}

#[tokio::test]
async fn private_backup_route_rejects_wrong_kind_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        local_member,
        1,
        FederationRestMessageKind::CatchUp,
        FederationAlephBftBackupRequest::new(4, 0),
        &peer_private,
    )
    .expect("wrong-kind backup request");

    let response = post_aleph_bft_backup(State(state.clone()), Json(request))
        .await
        .expect_err("wrong message kind must fail");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "wrong-kind backup requests must not persist the sender sequence"
    );
}

#[tokio::test]
async fn private_journal_route_rejects_wrong_message_kind_before_sequence_persistence() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = signed_catch_up_request_with_kind(
        &public_config,
        &peer_private,
        local_member,
        1,
        FederationRestMessageKind::Status,
        FederationJournalCatchUpRequest::new(0),
    );

    let response = post_journal(State(state.clone()), Json(request))
        .await
        .expect_err("wrong message kind must fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("persisted sequences");
    assert!(
        sequences.is_empty(),
        "wrong-kind catch-up requests must not persist the sender sequence"
    );
}

fn melt_quote_operation_with_lookup_id(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
    request_lookup_id: cdk::cdk_payment::PaymentIdentifier,
) -> FederationOperationEnvelope {
    let cdk::MeltQuoteRequest::Bolt11(bolt11) = &request else {
        panic!("federation route tests only construct bolt11 melt quotes");
    };
    let response = cdk::MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: quote.clone(),
        amount: Amount::from(
            bolt11
                .request
                .amount_milli_satoshis()
                .unwrap_or(1_000)
                .div_ceil(1_000),
        ),
        fee_reserve: Amount::from(1),
        state: MeltQuoteState::Unpaid,
        expiry: 4_000_000_000,
        payment_preimage: None,
        change: None,
        request: Some(bolt11.request.to_string()),
        unit: Some(bolt11.unit.clone()),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    });
    config.melt_quote_operation(
        quote,
        request,
        response,
        cdk::federation::FederationMeltQuoteMetadata {
            created_time: 1_700_000_000,
            request_lookup_id: Some(request_lookup_id),
            extra_json: None,
            estimated_blocks: None,
        },
    )
}

async fn submit_observer_test_melt(
    state: &MintState,
    description: FakeInvoiceDescription,
    label: &str,
) {
    let quote_id = QuoteId::new();
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(
            1_000,
            serde_json::to_string(&description).expect("description"),
        ),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let cdk::MeltQuoteRequest::Bolt11(bolt11) = &request else {
        panic!("bolt11 request");
    };
    let request_lookup_id =
        cdk::cdk_payment::PaymentIdentifier::PaymentHash(*bolt11.request.payment_hash().as_ref());
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let quote_envelope =
        melt_quote_operation_with_lookup_id(&config, quote_id.clone(), request, request_lookup_id);
    state
        .submit_federation_envelope(quote_envelope)
        .await
        .expect("submit melt quote");

    let proofs = vec![
        proof_for_amount(state, 1, &format!("{label}-one")),
        proof_for_amount(state, 1, &format!("{label}-two")),
    ];
    let melt_envelope = config
        .melt_operation(cdk::nuts::MeltRequest::new(quote_id, proofs, None))
        .expect("melt operation");
    state
        .submit_federation_envelope(melt_envelope)
        .await
        .expect("submit melt");
}

#[tokio::test]
async fn poisoned_melt_quote_cannot_starve_observer_round() {
    let (state, _, _) = federation_route_state().await;

    // The first quote's backend rejects every attempt and every status check.
    submit_observer_test_melt(
        &state,
        FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: true,
            check_err: true,
        },
        "poisoned",
    )
    .await;
    // The second quote's backend settles normally.
    submit_observer_test_melt(
        &state,
        FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        },
        "healthy",
    )
    .await;

    let pending_quotes = state
        .mint
        .localstore()
        .get_melt_quotes()
        .await
        .expect("melt quotes")
        .into_iter()
        .filter(|quote| quote.state == MeltQuoteState::Pending)
        .map(|quote| quote.id)
        .collect::<Vec<_>>();
    assert_eq!(pending_quotes.len(), 2);
    let poisoned_quote = pending_quotes[0].clone();
    let healthy_quote = pending_quotes[1].clone();

    // The single sequential observer worker must isolate the poisoned quote
    // and still observe the healthy one; the poisoned quote must remain
    // pending rather than being translated into a terminal failure.
    let observer = super::payment_observation::FederationPaymentObserver::default();
    let submitted = observer
        .run_round(&state)
        .await
        .expect("poisoned quote must not abort the observer round");
    assert!(submitted >= 1, "healthy quote must be observed");

    let healthy_observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MeltQuote,
            &healthy_quote.to_string(),
        )
        .await
        .expect("healthy observation rows");
    assert!(
        healthy_observations.iter().any(|record| matches!(
            &record.observation,
            cdk::federation::FederationPaymentObservation::MeltQuote(observation)
                if observation.state == MeltQuoteState::Paid
        )),
        "healthy quote must have a paid observation"
    );

    let poisoned_after = state
        .mint
        .localstore()
        .get_melt_quote(&poisoned_quote)
        .await
        .expect("poisoned quote lookup")
        .expect("poisoned quote");
    assert_eq!(
        poisoned_after.state,
        MeltQuoteState::Pending,
        "the poisoned quote must remain pending, never a terminal failure"
    );
}
