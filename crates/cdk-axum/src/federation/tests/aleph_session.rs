use super::*;

#[tokio::test]
async fn aleph_bft_live_session_startup_decision_spawns_for_federation_member() {
    let (public_config, local_private, _) = transport_config();
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let aleph_bft_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db")),
        &seed,
    )
    .await;
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&aleph_bft_state)
            .await
            .expect("AlephBFT decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 0,
        }
    );
}

#[tokio::test]
async fn aleph_bft_live_session_start_installs_inbound_queue() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    let handle = start_federation_aleph_bft_live_session_with_config(
        &state,
        FederationPeerSyncConfig::default(),
        CdkAlephBftNetworkPumpConfig::default(),
    )
    .await
    .expect("start live session")
    .expect("live session handle");

    assert!(state
        .federation
        .aleph_bft_network_inbound
        .read()
        .await
        .is_some());
    assert!(state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .is_some());
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);
    let stored_handle = state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .clone()
        .expect("stored live session handle");
    assert!(Arc::ptr_eq(&handle, &stored_handle));
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("already-started decision"),
        FederationAlephBftLiveSessionStartupDecision::AlreadyStarted
    );
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal")
        .is_empty());
}

#[tokio::test]
async fn aleph_bft_session_supervisor_partial_startup_failure_publishes_no_tasks_or_inbound() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_pre_spawn_failure();

    start_federation_aleph_bft_live_session_with_options_and_transport(
        &state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect_err("injected startup failure");

    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);
}

#[tokio::test]
async fn aleph_bft_session_supervisor_halts_and_cleans_up_after_root_task_panic() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_session_task();
    let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
        &state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start panicking live session")
    .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ) && !state.federation_aleph_bft_live_session_active().await
                && !state.federation_aleph_bft_network_inbound_active().await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("panicking session cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_session_tasks_wait_for_supervisor_publication_before_activation() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let publication_reached = Arc::new(tokio::sync::Notify::new());
    let publication_release = Arc::new(tokio::sync::Notify::new());
    let root_task_activated = Arc::new(AtomicBool::new(false));
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_session_task()
            .with_publication_pause(
                Arc::clone(&publication_reached),
                Arc::clone(&publication_release),
            )
            .with_root_task_activation_observer(Arc::clone(&root_task_activated));
    let observed_state = state.clone();
    let start_state = state;
    let start_task = tokio::spawn(async move {
        start_federation_aleph_bft_live_session_with_options_and_transport(
            &start_state,
            FederationPeerSyncConfig::default(),
            options,
            RouterFederationTransportRegistry::default(),
        )
        .await
    });

    time::timeout(Duration::from_secs(5), publication_reached.notified())
        .await
        .expect("startup reached the pre-publication barrier");
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(
        !root_task_activated.load(Ordering::SeqCst),
        "the root task must remain gated while its supervisor is constructing"
    );
    assert!(!start_task.is_finished());
    assert!(
        !observed_state
            .federation_aleph_bft_live_session_active()
            .await
    );
    assert!(
        !observed_state
            .federation_aleph_bft_network_inbound_active()
            .await
    );

    publication_release.notify_one();
    let supervisor = time::timeout(Duration::from_secs(5), start_task)
        .await
        .expect("startup completed after publication was released")
        .expect("startup task")
        .expect("published session")
        .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if root_task_activated.load(Ordering::SeqCst)
                && matches!(
                    observed_state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                )
                && !observed_state
                    .federation_aleph_bft_live_session_active()
                    .await
                && !observed_state
                    .federation_aleph_bft_network_inbound_active()
                    .await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("post-publication root panic cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_session_halt_during_construction_aborts_without_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let publication_reached = Arc::new(tokio::sync::Notify::new());
    let publication_release = Arc::new(tokio::sync::Notify::new());
    let root_task_activated = Arc::new(AtomicBool::new(false));
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_publication_pause(
                Arc::clone(&publication_reached),
                Arc::clone(&publication_release),
            )
            .with_root_task_activation_observer(Arc::clone(&root_task_activated));
    let observed_state = state.clone();
    let start_state = state;
    let start_task = tokio::spawn(async move {
        start_federation_aleph_bft_live_session_with_options_and_transport(
            &start_state,
            FederationPeerSyncConfig::default(),
            options,
            RouterFederationTransportRegistry::default(),
        )
        .await
    });

    time::timeout(Duration::from_secs(5), publication_reached.notified())
        .await
        .expect("startup reached the pre-publication barrier");
    halt_current_federation_aleph_bft_session(&observed_state, "test construction halt").await;
    publication_release.notify_one();

    let err = time::timeout(Duration::from_secs(5), start_task)
        .await
        .expect("halted startup completed")
        .expect("startup task")
        .expect_err("halt epoch change must reject constructing supervisor publication");
    assert!(err.to_string().contains("halted before publication"));
    assert!(!root_task_activated.load(Ordering::SeqCst));
    assert!(
        !observed_state
            .federation_aleph_bft_live_session_active()
            .await
    );
    assert!(
        !observed_state
            .federation_aleph_bft_network_inbound_active()
            .await
    );
}

#[tokio::test]
async fn aleph_bft_session_supervisor_halts_after_network_pump_panic() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let options =
        FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig::default())
            .with_panicking_network_pump_task();
    let supervisor = start_federation_aleph_bft_live_session_with_options_and_transport(
        &state,
        FederationPeerSyncConfig::default(),
        options,
        RouterFederationTransportRegistry::default(),
    )
    .await
    .expect("start live session with panicking network pump")
    .expect("live session supervisor");

    time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ) && !state.federation_aleph_bft_live_session_active().await
                && !state.federation_aleph_bft_network_inbound_active().await
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("network pump panic cleanup");

    assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
}

#[tokio::test]
async fn aleph_bft_non_retryable_finalized_payload_failure_halts_exact_generation() {
    let options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    )
    .with_finalized_payload_apply_failure();
    let session = start_two_member_aleph_bft_live_session(options)
        .await
        .expect("start apply-failure live session");
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&session.first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = session.first_state.clone();
    let submit_task = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    for state in [&session.first_state, &session.second_state] {
        time::timeout(Duration::from_secs(10), async {
            loop {
                if matches!(
                    state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                ) && !state.federation_aleph_bft_live_session_active().await
                    && !state.federation_aleph_bft_network_inbound_active().await
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("non-retryable apply failure cleanup");
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await
            .expect("journal lookup after apply failure")
            .is_none());
        let rejected = state
            .submit_federation_envelope(mint_quote_envelope(state, &QuoteId::new()))
            .await
            .expect_err("halted generation must reject public submission");
        assert!(matches!(
            rejected,
            cdk::Error::Federation(FederationError::NodeHalted { .. })
        ));
    }
    submit_task.abort();
    let _ = submit_task.await;
}

async fn assert_balance_overflow_rejection_converged(
    state: &MintState,
    invalid_operation_id: FederationOperationId,
    quote_ids: &[QuoteId],
) {
    assert!(
        state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(invalid_operation_id)
            .await
            .expect("invalid operation journal lookup")
            .is_none(),
        "invalid operation must not become a journal entry"
    );
    let invalid_status = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items lookup")
        .into_iter()
        .find(|item| item.operation_id == Some(invalid_operation_id))
        .map(|item| item.status);
    assert_eq!(
        invalid_status,
        Some(FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation),
        "invalid operation must be recorded as a per-item rejection"
    );
    for quote_id in quote_ids {
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(quote_id)
            .await
            .expect("mint quote lookup")
            .is_some());
    }
}

#[tokio::test]
async fn byzantine_balance_overflow_operation_does_not_halt_finalized_application() {
    let live_options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 256,
            max_peer_deliveries_per_message: 256,
        },
        500,
    );
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("start five-member live session");

    // Partition the fifth member before any operation so that its later rejoin
    // is a genuine catch-up of the Byzantine unit from peer history.
    *session.states[4]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[4]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(session.states[0].clone()),
    );
    let fifth_handle = session._handles.remove(4);
    drop(fifth_handle);

    let byzantine_state = session.states[0].clone();
    let honest_states: Vec<MintState> = session.states[1..4].to_vec();

    // One Byzantine member signs a structurally valid mint operation whose
    // output amounts overflow the operation-local balance audit, then queues it
    // directly into its own AlephBFT data provider, bypassing public admission
    // validation. A valid mint quote operation queued behind it in the same
    // atomic batch lands in the same unit.
    let config = byzantine_state
        .mint
        .federation_config()
        .expect("federation config");
    let keyset_id = config.public_config().keysets[0].id;
    let overflow_output = |label: &[u8]| {
        BlindedMessage::new(
            Amount::from(u64::MAX),
            keyset_id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
        )
    };
    let invalid_envelope = config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![
                    overflow_output(b"byzantine balance overflow output 1"),
                    overflow_output(b"byzantine balance overflow output 2"),
                ],
                signature: None,
            }),
        },
    ));
    let invalid_operation_id = invalid_envelope
        .operation_id()
        .expect("invalid operation id");
    let same_unit_quote_id = QuoteId::new();
    let same_unit_envelope = mint_quote_envelope(&byzantine_state, &same_unit_quote_id);
    let same_unit_operation_id = same_unit_envelope
        .operation_id()
        .expect("same-unit operation id");
    {
        let consensus = byzantine_state.federation.consensus().expect("consensus");
        let mut consensus = consensus.lock().await;
        assert_eq!(
            consensus
                .submit_operation(invalid_envelope)
                .expect("queue Byzantine overflow operation"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(
            consensus
                .submit_operation(same_unit_envelope)
                .expect("queue same-unit valid operation"),
            FederationConsensusItemAdmission::Accepted
        );
    }

    // An honest member submits one valid operation through the public path so
    // it lands in a unit after the Byzantine unit. The submit may return a
    // bounded pending-finalization timeout under load; per-member terminal
    // waits below are the application proof.
    let next_unit_quote_id = QuoteId::new();
    let honest_submit_state = honest_states[0].clone();
    let next_unit_envelope = mint_quote_envelope(&honest_submit_state, &next_unit_quote_id);
    let next_unit_operation_id = next_unit_envelope
        .operation_id()
        .expect("next-unit operation id");
    let next_unit_submit = tokio::spawn(async move {
        submit_live_operation_allowing_pending_timeout(&honest_submit_state, next_unit_envelope)
            .await
    });

    // Wait until the valid same-unit operation reaches a terminal state on the
    // first honest member, or any honest member halts first.
    let mut same_unit_status = None;
    let mut halt_reason = None;
    time::timeout(Duration::from_secs(20), async {
        loop {
            for state in &honest_states {
                if let Some(crate::FederationSyncStatus::Halted { reason }) =
                    state.federation_sync_status().await
                {
                    halt_reason = Some(reason);
                    break;
                }
            }
            if halt_reason.is_some() {
                break;
            }
            if let Some(entry) = honest_states[0]
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(same_unit_operation_id)
                .await
                .expect("same-unit journal lookup")
            {
                if matches!(
                    entry.status(),
                    FederationOperationStatus::Applied | FederationOperationStatus::Rejected
                ) {
                    same_unit_status = Some(entry.status());
                    break;
                }
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Byzantine unit reached a decision before timeout");

    assert!(
        halt_reason.is_none(),
        "one Byzantine balance-overflow operation halted an honest member: {}",
        halt_reason.as_deref().unwrap_or("unknown halt")
    );
    assert_eq!(
        same_unit_status,
        Some(FederationOperationStatus::Applied),
        "valid operation in the Byzantine unit must still apply"
    );

    let submitted_next_unit_operation_id = time::timeout(Duration::from_secs(30), next_unit_submit)
        .await
        .expect("next-unit submission completed before timeout")
        .expect("next-unit submit task");
    assert_eq!(submitted_next_unit_operation_id, next_unit_operation_id);

    let live_quote_ids = [same_unit_quote_id.clone(), next_unit_quote_id.clone()];
    for state in &honest_states {
        assert_eq!(
            wait_for_terminal_operation(state, same_unit_operation_id).await,
            FederationOperationStatus::Applied,
            "honest member must apply the valid same-unit operation"
        );
        assert_eq!(
            wait_for_terminal_operation(state, next_unit_operation_id).await,
            FederationOperationStatus::Applied,
            "honest member must apply the valid next-unit operation"
        );
        assert!(!matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ));
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);
        assert_balance_overflow_rejection_converged(state, invalid_operation_id, &live_quote_ids)
            .await;
    }

    let reference_journal = journal_convergence_projection(
        &session.states[0]
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("reference journal lookup"),
    );
    for state in &honest_states {
        assert_eq!(
            journal_convergence_projection(
                &state
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("honest journal lookup"),
            ),
            reference_journal,
            "honest members must converge on the same journal"
        );
    }

    // Restart one honest member from its durable state and prove the rejection
    // and journal survive replay before it rejoins live consensus.
    *session.states[3]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[3]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    session.transport.insert(
        session.federation_api_urls[3].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(session.states[0].clone()),
    );
    let third_handle = session._handles.remove(3);
    drop(third_handle);
    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[3]).await;
    assert_balance_overflow_rejection_converged(
        &session.states[3],
        invalid_operation_id,
        &live_quote_ids,
    )
    .await;
    let third_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &session.states[3],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart honest member after Byzantine rejection")
    .expect("restarted honest live session handle");
    session._handles.push(third_handle);
    session.transport.insert(
        session.federation_api_urls[3].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[3].clone()),
    );
    for state in &mut session.states[..4] {
        state.federation.member_mut_for_test().peer_sync_config = session.sync_config;
    }

    let post_restart_quote_id = QuoteId::new();
    let post_restart_envelope = mint_quote_envelope(&session.states[1], &post_restart_quote_id);
    let post_restart_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[1], post_restart_envelope)
            .await;
    for state in &session.states[..4] {
        assert_eq!(
            wait_for_terminal_operation(state, post_restart_operation_id).await,
            FederationOperationStatus::Applied,
            "restarted member and peers must apply a post-restart operation"
        );
    }

    // Rejoin the partitioned member through peer catch-up and prove it
    // reproduces the same rejection, journal, and materialized quotes.
    session.states[4]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("lagging member must fail closed before catch-up");
    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[4]).await;
    session.transport.insert(
        session.federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[4].clone()),
    );
    sync_and_certify_partitioned_member(&session, 4, live_options).await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect("caught-up member is ready");

    let all_operation_ids = [
        same_unit_operation_id,
        next_unit_operation_id,
        post_restart_operation_id,
    ];
    for operation_id in all_operation_ids {
        assert_eq!(
            wait_for_terminal_operation(&session.states[4], operation_id).await,
            FederationOperationStatus::Applied,
            "caught-up member must apply every valid neighboring operation"
        );
    }
    let all_quote_ids = [
        same_unit_quote_id,
        next_unit_quote_id,
        post_restart_quote_id,
    ];
    assert_balance_overflow_rejection_converged(
        &session.states[4],
        invalid_operation_id,
        &all_quote_ids,
    )
    .await;

    let reference_journal = journal_convergence_projection(
        &session.states[0]
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("final reference journal lookup"),
    );
    for state in &session.states {
        assert_eq!(
            journal_convergence_projection(
                &state
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("converged journal lookup"),
            ),
            reference_journal,
            "all members must converge on the same journal after catch-up"
        );
        assert_balance_overflow_rejection_converged(state, invalid_operation_id, &all_quote_ids)
            .await;
    }
}

#[tokio::test]
async fn byzantine_paid_melt_observation_without_saga_rejects_item_without_halt() {
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    ))
    .await
    .expect("start two-member live session");

    // Create a real melt quote through live consensus. No melt operation is
    // ever submitted, so no member can hold a pending saga for it.
    let quote_id = QuoteId::new();
    let melt_quote_envelope = test_melt_quote_operation(
        session
            .second_state
            .mint
            .federation_config()
            .expect("federation config"),
        quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(1_000, "db-001 trap quote".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_quote_operation_id = melt_quote_envelope.operation_id().expect("quote op id");
    submit_live_operation_allowing_pending_timeout(&session.second_state, melt_quote_envelope)
        .await;
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, melt_quote_operation_id).await,
            FederationOperationStatus::Applied
        );
    }

    // One Byzantine member queues a structurally valid paid observation for
    // that saga-less quote directly into its own data provider, bypassing
    // public admission. The observation's pure content is valid; only its
    // ordered-state dependency (the missing melt saga) is invalid.
    let byzantine_envelope = melt_payment_observation_envelope(
        &session.first_state,
        &quote_id,
        MeltQuoteState::Paid,
        Some("byzantine preimage".to_string()),
    );
    let byzantine_operation_id = byzantine_envelope
        .operation_id()
        .expect("observation op id");
    {
        let consensus = session
            .first_state
            .federation
            .consensus()
            .expect("consensus");
        let mut consensus = consensus.lock().await;
        assert_eq!(
            consensus
                .submit_operation(byzantine_envelope)
                .expect("queue Byzantine paid observation"),
            FederationConsensusItemAdmission::Accepted
        );
    }

    // A valid mint quote submitted through the live path drives unit
    // activity and lands in a unit after the Byzantine one.
    let trailing_quote_id = QuoteId::new();
    let trailing_envelope = mint_quote_envelope(&session.second_state, &trailing_quote_id);
    let trailing_operation_id = trailing_envelope.operation_id().expect("trailing op id");
    let trailing_state = session.second_state.clone();
    let trailing_submit = tokio::spawn(async move {
        submit_live_operation_allowing_pending_timeout(&trailing_state, trailing_envelope).await
    });

    // The stateful-invalid observation must be a canonical per-item rejection
    // — never a member halt — and the valid trailing operation must apply on
    // both members.
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, byzantine_operation_id).await,
            FederationOperationStatus::Rejected,
            "Byzantine saga-less paid observation must be a canonical per-item rejection"
        );
        assert_eq!(
            wait_for_terminal_operation(state, trailing_operation_id).await,
            FederationOperationStatus::Applied,
            "valid operation finalized after the Byzantine item must still apply"
        );
        assert!(
            !matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ),
            "one Byzantine stateful-invalid operation must not halt a member"
        );
        assert!(state.federation_aleph_bft_live_session_active().await);
    }
    trailing_submit.abort();
    let _ = trailing_submit.await;
}

#[tokio::test]
async fn duplicate_pending_and_paid_melts_reject_without_halting_later_finality() {
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    ))
    .await
    .expect("start two-member live session");
    let runtime_config = session
        .first_state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(1_000, String::new()),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let cdk::MeltQuoteRequest::Bolt11(bolt11_request) = &request else {
        unreachable!("test request is bolt11");
    };
    let request_lookup_id =
        PaymentIdentifier::PaymentHash(*bolt11_request.request.payment_hash().as_ref());
    let original_quote = QuoteId::new();
    let pending_duplicate_quote = QuoteId::new();
    let duplicate_quote = QuoteId::new();

    for quote in [&original_quote, &pending_duplicate_quote, &duplicate_quote] {
        let quote_envelope = test_melt_quote_operation_with_lookup_id(
            &runtime_config,
            quote.clone(),
            request.clone(),
            request_lookup_id.clone(),
        );
        let operation_id =
            submit_live_operation_allowing_pending_timeout(&session.first_state, quote_envelope)
                .await;
        for state in [&session.first_state, &session.second_state] {
            assert_eq!(
                wait_for_terminal_operation(state, operation_id).await,
                FederationOperationStatus::Applied
            );
        }
    }

    let original_inputs = vec![
        proof_for_amount(&session.first_state, 1, "duplicate-paid-original-one"),
        proof_for_amount(&session.first_state, 1, "duplicate-paid-original-two"),
    ];
    let original_input_ys = original_inputs
        .iter()
        .map(|proof| proof.y().expect("original proof y"))
        .collect::<Vec<_>>();
    let original_envelope = runtime_config
        .melt_operation(cdk::nuts::MeltRequest::new(
            original_quote.clone(),
            original_inputs,
            None,
        ))
        .expect("original melt envelope");
    let original_operation_id = original_envelope
        .operation_id()
        .expect("original operation id");
    let pending_duplicate_inputs = vec![
        proof_for_amount(&session.first_state, 1, "duplicate-pending-second-one"),
        proof_for_amount(&session.first_state, 1, "duplicate-pending-second-two"),
    ];
    let pending_duplicate_input_ys = pending_duplicate_inputs
        .iter()
        .map(|proof| proof.y().expect("pending duplicate proof y"))
        .collect::<Vec<_>>();
    let pending_duplicate_envelope = runtime_config
        .melt_operation(cdk::nuts::MeltRequest::new(
            pending_duplicate_quote.clone(),
            pending_duplicate_inputs,
            None,
        ))
        .expect("pending duplicate melt envelope");
    let pending_duplicate_operation_id = pending_duplicate_envelope
        .operation_id()
        .expect("pending duplicate operation id");
    {
        let consensus = session
            .first_state
            .federation
            .consensus()
            .expect("consensus");
        let mut consensus = consensus.lock().await;
        assert_eq!(
            consensus
                .submit_operation(original_envelope)
                .expect("queue original melt"),
            FederationConsensusItemAdmission::Accepted
        );
        assert_eq!(
            consensus
                .submit_operation(pending_duplicate_envelope)
                .expect("queue concurrent duplicate melt"),
            FederationConsensusItemAdmission::Accepted
        );
    }
    let pending_driver_quote = QuoteId::new();
    let pending_driver_operation_id = submit_live_operation_allowing_pending_timeout(
        &session.second_state,
        mint_quote_envelope(&session.second_state, &pending_driver_quote),
    )
    .await;
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, original_operation_id).await,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            wait_for_terminal_operation(state, pending_duplicate_operation_id).await,
            FederationOperationStatus::Rejected
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&pending_duplicate_quote)
                .await
                .expect("pending duplicate quote lookup")
                .expect("pending duplicate quote")
                .state,
            MeltQuoteState::Unpaid
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&pending_duplicate_input_ys)
                .await
                .expect("pending duplicate proof states"),
            vec![None, None]
        );
        assert!(
            !matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ),
            "concurrent duplicate melt must not halt a member"
        );
        assert_eq!(
            wait_for_terminal_operation(state, pending_driver_operation_id).await,
            FederationOperationStatus::Applied
        );
    }

    let actor_observation =
        submit_melt_payment_observation_from_backend(&session.first_state, &original_quote).await;
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, actor_observation.operation_id).await,
            FederationOperationStatus::Applied
        );
    }
    let follower_observation = matching_melt_payment_observation_envelope(
        &session.second_state,
        &actor_observation.observation,
    );
    let follower_observation_id =
        submit_live_operation_allowing_pending_timeout(&session.second_state, follower_observation)
            .await;
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, follower_observation_id).await,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&original_quote)
                .await
                .expect("original quote lookup")
                .expect("original quote")
                .state,
            MeltQuoteState::Paid
        );
    }

    let duplicate_inputs = vec![
        proof_for_amount(&session.first_state, 1, "duplicate-paid-second-one"),
        proof_for_amount(&session.first_state, 1, "duplicate-paid-second-two"),
    ];
    let duplicate_input_ys = duplicate_inputs
        .iter()
        .map(|proof| proof.y().expect("duplicate proof y"))
        .collect::<Vec<_>>();
    let duplicate_envelope = runtime_config
        .melt_operation(cdk::nuts::MeltRequest::new(
            duplicate_quote.clone(),
            duplicate_inputs,
            None,
        ))
        .expect("duplicate melt envelope");
    let duplicate_operation_id = duplicate_envelope
        .operation_id()
        .expect("duplicate operation id");
    let duplicate_retry = duplicate_envelope.clone();
    let submit_state = session.first_state.clone();
    let duplicate_submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(duplicate_envelope)
            .await
    });
    let duplicate_error = time::timeout(Duration::from_secs(20), duplicate_submit)
        .await
        .expect("duplicate melt submission timeout")
        .expect("duplicate melt submission task")
        .expect_err("duplicate paid melt must be rejected");
    assert!(matches!(duplicate_error, cdk::Error::RequestAlreadyPaid));

    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, duplicate_operation_id).await,
            FederationOperationStatus::Rejected
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&duplicate_quote)
                .await
                .expect("duplicate quote lookup")
                .expect("duplicate quote")
                .state,
            MeltQuoteState::Unpaid
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&duplicate_input_ys)
                .await
                .expect("duplicate proof states"),
            vec![None, None]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&original_input_ys)
                .await
                .expect("original proof states"),
            vec![Some(cdk::nuts::State::Spent), Some(cdk::nuts::State::Spent)]
        );
        assert!(
            !matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            ),
            "a duplicate paid melt must not halt a member"
        );
        assert!(state.federation_aleph_bft_live_session_active().await);
    }

    assert!(matches!(
        session
            .first_state
            .submit_federation_envelope_to_consensus(duplicate_retry)
            .await,
        Err(cdk::Error::RequestAlreadyPaid)
    ));

    let trailing_quote = QuoteId::new();
    let trailing_envelope = mint_quote_envelope(&session.second_state, &trailing_quote);
    let trailing_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.second_state, trailing_envelope)
            .await;
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, trailing_operation_id).await,
            FederationOperationStatus::Applied
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&trailing_quote)
            .await
            .expect("trailing quote lookup")
            .is_some());
        let (restarted, _) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("restart runtime from rejected melt history");
        let restarted = restarted.expect("restarted consensus");
        assert_eq!(
            restarted
                .lock()
                .await
                .journal()
                .entry_by_operation_id(duplicate_operation_id)
                .expect("restarted duplicate journal entry")
                .status(),
            FederationOperationStatus::Rejected
        );
        assert_eq!(
            restarted
                .lock()
                .await
                .journal()
                .entry_by_operation_id(pending_duplicate_operation_id)
                .expect("restarted pending duplicate journal entry")
                .status(),
            FederationOperationStatus::Rejected
        );
    }
}

#[tokio::test]
async fn stateful_invalid_chain_deposit_rejects_item_without_halt() {
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

    // A deposit observation whose pure content is valid but whose allocation
    // reference has no accepted projection in ordered state.
    let deposit = cdk::federation::FederationDepositObservation {
        allocation: cdk::federation::FederationAllocatedAddressRef {
            application_id: cdk::federation::FederationFrostApplicationId::from_bytes([7; 32])
                .expect("application id"),
            application_key_epoch: 2,
            quote: "unknown-allocation-quote".to_string(),
            keychain: cdk::federation::FederationAddressKeychain::External,
            derivation_index: 4,
        },
        outpoint: cdk_common::bitcoin::OutPoint {
            txid: cdk_common::bitcoin::Txid::from_raw_hash(
                cdk_common::bitcoin::hashes::Hash::from_byte_array([3u8; 32]),
            ),
            vout: 0,
        },
        txout: cdk_common::bitcoin::TxOut {
            value: cdk_common::bitcoin::Amount::from_sat(50_000),
            script_pubkey: cdk_common::bitcoin::ScriptBuf::from_bytes(vec![0x51]),
        },
        confirmation: None,
        observation_epoch: 0,
        observer: local_private.member_id,
        source: cdk::federation::FederationChainSourceDescriptor::new(
            cdk::federation::FederationChainSourceScope::Independent,
            "source-1",
        )
        .expect("source descriptor"),
    };
    let config = state.mint.federation_config().expect("federation config");
    let deposit_envelope =
        config.operation_envelope(cdk::federation::FederationOperation::ChainObservation(
            cdk::federation::FederationChainObservation::Deposit(deposit),
        ));
    let deposit_operation_id = deposit_envelope.operation_id().expect("deposit op id");
    let trailing_quote_id = QuoteId::new();
    let trailing_envelope = mint_quote_envelope(&state, &trailing_quote_id);
    let trailing_operation_id = trailing_envelope.operation_id().expect("trailing op id");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(deposit_envelope),
            FederationConsensusItem::operation(trailing_envelope),
        ],
    )
    .expect("stateful-invalid deposit unit");
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("stateful-invalid deposit must not abort unit application");

    let deposit_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(deposit_operation_id)
        .await
        .expect("deposit journal lookup")
        .expect("deposit journal entry");
    assert_eq!(
        deposit_entry.status(),
        FederationOperationStatus::Rejected,
        "stateful-invalid deposit must be a canonical per-item rejection"
    );
    let trailing_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(trailing_operation_id)
        .await
        .expect("trailing journal lookup")
        .expect("trailing journal entry");
    assert_eq!(
        trailing_entry.status(),
        FederationOperationStatus::Applied,
        "the valid operation in the same unit must still apply"
    );
    assert!(
        !matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ),
        "one stateful-invalid deposit must not halt the member"
    );
}

#[test]
fn federation_apply_content_errors_are_definitive_but_corruption_errors_are_not() {
    use cdk::federation::FederationError;

    for error in [
        FederationError::OperationStateInvalid {
            reason: "x".to_string(),
        },
        FederationError::ChainObservationInvalid {
            reason: "x".to_string(),
        },
        FederationError::AddressAllocationInvalid {
            reason: "x".to_string(),
        },
    ] {
        assert!(
            cdk::Error::from(error).is_definitive_failure(),
            "deterministic apply-time content errors must be definitive"
        );
    }
    assert!(
        !cdk::Error::from(FederationError::ChainObservationStateInvalid {
            reason: "x".to_string(),
        })
        .is_definitive_failure(),
        "genuine local state corruption must stay halt-worthy"
    );
    assert!(
        !cdk::Error::Internal.is_definitive_failure(),
        "free-form internal errors must stay halt-worthy"
    );
}

#[tokio::test]
async fn aleph_bft_automatic_rotation_failure_halts_without_live_resources() {
    let options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        6,
    )
    .with_automatic_rotation_limit(1)
    .with_automatic_rotation_start_failure();
    let session = start_two_member_aleph_bft_live_session(options)
        .await
        .expect("start rotation-failure live session");
    let rotation_activity = FederationOperationId::from_bytes([203; 32]);

    time::timeout(Duration::from_secs(10), async {
        loop {
            for state in [&session.first_state, &session.second_state] {
                if let Some(handle) = state.federation.aleph_bft_live_session.read().await.clone() {
                    handle.mark_peer_activity(rotation_activity);
                }
            }
            let mut all_halted_and_stopped = true;
            for state in [&session.first_state, &session.second_state] {
                all_halted_and_stopped &= matches!(
                    state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Halted { .. })
                );
                all_halted_and_stopped &= !state.federation_aleph_bft_live_session_active().await;
                all_halted_and_stopped &=
                    !state.federation_aleph_bft_network_inbound_active().await;
            }
            if all_halted_and_stopped {
                break;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("automatic rotation failure did not halt both members");

    for state in [&session.first_state, &session.second_state] {
        assert!(!state.federation_aleph_bft_live_session_active().await);
        assert!(!state.federation_aleph_bft_network_inbound_active().await);
        let rejected = state
            .submit_federation_envelope(mint_quote_envelope(state, &QuoteId::new()))
            .await
            .expect_err("rotation-failure halt must reject public submission");
        assert!(matches!(
            rejected,
            cdk::Error::Federation(FederationError::NodeHalted { .. })
        ));
    }
}

#[tokio::test]
async fn aleph_bft_live_session_persists_and_recovers_empty_active_session() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .begin_aleph_bft_session(1)
        .expect("begin empty session one");

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
    );
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let stale_pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &state,
        sync_config,
        stale_pump_config,
        transport,
    )
    .await
    .expect("start empty session")
    .expect("live session handle");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_active_session()
            .await
            .expect("persisted active session"),
        Some(1)
    );
    drop(handle);

    let mut restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (consensus, sequences) = crate::create_federation_runtime_state(&restarted_state.mint)
        .await
        .expect("recover active empty session");
    restarted_state.federation.member_mut_for_test().consensus =
        consensus.expect("configured member handle");
    restarted_state.federation.member_mut_for_test().sequences =
        sequences.expect("configured member handle");
    let status = restarted_state
        .federation_aleph_bft_status()
        .await
        .expect("recovered AlephBFT status");
    assert_eq!(status.session, 1);
    assert_eq!(status.finalized_item_count, 0);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&restarted_state)
            .await
            .expect("startup decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 1,
        }
    );
}

#[tokio::test]
async fn aleph_bft_restart_finalizes_atomically_rebound_pending_operations() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote);
    let second_envelope = mint_quote_envelope(&state, &second_quote);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let second_operation_id = second_envelope.operation_id().expect("second operation id");

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin crash fixture tx");
    tx.add_federation_aleph_bft_pending_operation(
        &cdk::federation::FederationAlephBftPendingOperationRecord {
            operation_id: first_operation_id,
            session_id: 0,
            envelope: first_envelope,
            ordered_group: None,
        },
    )
    .await
    .expect("persist first older-session operation");
    tx.add_federation_aleph_bft_pending_operation(
        &cdk::federation::FederationAlephBftPendingOperationRecord {
            operation_id: second_operation_id,
            session_id: 0,
            envelope: second_envelope,
            ordered_group: None,
        },
    )
    .await
    .expect("persist second older-session operation");
    tx.set_federation_aleph_bft_active_session(1)
        .await
        .expect("atomically advance and rebind pending operations");
    tx.commit().await.expect("commit crash fixture");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("recover and rebind pending operations");
    state.federation.member_mut_for_test().consensus = consensus.expect("configured member handle");
    state.federation.member_mut_for_test().sequences = sequences.expect("configured member handle");
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("recovered status")
            .queued_item_count,
        2
    );
    assert!(db
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("older pending rows")
        .is_empty());
    assert_eq!(
        db.get_federation_aleph_bft_pending_operations(1)
            .await
            .expect("rebound pending rows")
            .len(),
        2
    );

    let unit = state
        .federation
        .consensus()
        .expect("recovered consensus")
        .lock()
        .await
        .next_aleph_bft_unit_payload()
        .expect("next recovered unit")
        .expect("recovered unit payload");
    state
        .federation
        .apply_finalized_unit(unit)
        .await
        .expect("finalize recovered pending operations");

    assert!(db
        .get_federation_aleph_bft_pending_operations(1)
        .await
        .expect("pending rows after finalization")
        .is_empty());
    for operation_id in [first_operation_id, second_operation_id] {
        assert_eq!(
            db.get_federation_journal_entry_by_operation_id(operation_id)
                .await
                .expect("journal lookup")
                .expect("recovered journal entry")
                .status(),
            FederationOperationStatus::Applied
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_session_rotates_to_next_empty_session_and_uses_new_network_pump() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let initial_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
        );
    let initial_handle = start_federation_aleph_bft_live_session_with_transport(
        &state,
        sync_config,
        pump_config,
        initial_transport.clone(),
    )
    .await
    .expect("start initial live session")
    .expect("initial live session handle");
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("initial AlephBFT status")
            .session,
        0
    );

    let rotated_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
        );
    let rotated_handle = rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
        &mut state,
        sync_config,
        pump_config,
        rotated_transport.clone(),
    )
    .await
    .expect("rotate live session")
    .expect("rotated live session handle");

    assert!(!Arc::ptr_eq(&initial_handle, &rotated_handle));
    let status = state
        .federation_aleph_bft_status()
        .await
        .expect("rotated AlephBFT status");
    assert_eq!(status.session, 1);
    assert_eq!(status.next_unit_index, 0);
    assert_eq!(status.next_unit_finalization_index, 0);
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_aleph_bft_active_session()
            .await
            .expect("persisted rotated active session"),
        Some(1)
    );
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("rotated startup decision"),
        FederationAlephBftLiveSessionStartupDecision::AlreadyStarted
    );
    let initial_request_count_after_rotation =
        initial_transport.aleph_bft_network_message_requests().len();

    let next_envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(next_envelope)
        .expect("queue rotated-session operation");
    time::timeout(
        Duration::from_secs(5),
        rotated_transport.wait_for_aleph_bft_network_message_request_matching(|_| true),
    )
    .await
    .expect("rotated-session network-message request");
    let requests = rotated_transport.aleph_bft_network_message_requests();

    assert!(
        requests
            .iter()
            .all(|request| request.payload.session_id == 1),
        "rotated live network pump must deliver only the new session"
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("rotated session network-message request verifies");
        assert_eq!(request.sender, local_private.member_id);
    }
    assert_eq!(
        initial_transport.aleph_bft_network_message_requests().len(),
        initial_request_count_after_rotation,
        "the old live-session pump must stay stopped after rotation"
    );

    let halt_epoch = state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst);
    halt_federation_aleph_bft_generation_for_test(&state, 0, initial_handle.generation()).await;
    assert_eq!(
        state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst),
        halt_epoch,
        "a stale generation failure must not publish a halt"
    );
    let installed = state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .clone()
        .expect("rotated generation remains installed");
    assert!(Arc::ptr_eq(&installed, &rotated_handle));
    assert!(state.federation_aleph_bft_network_inbound_active().await);

    drop(initial_handle);
    drop(rotated_handle);
}

#[tokio::test]
async fn aleph_bft_live_session_rotation_refuses_non_empty_pending_mempool() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &state,
        sync_config,
        pump_config,
        transport,
    )
    .await
    .expect("start live session")
    .expect("live session handle");

    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(envelope)
        .expect("queue pending operation");

    let rotated_transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(
            FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
        );
    let err = rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
        &mut state,
        sync_config,
        pump_config,
        rotated_transport,
    )
    .await
    .expect_err("non-empty mempool must block live-session rotation");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::AlephBftSessionRotationBlocked {
            session: 0,
            pending_items: 1,
        })
    ));
    let status = state
        .federation_aleph_bft_status()
        .await
        .expect("unrotated AlephBFT status");
    assert_eq!(status.session, 0);
    assert_eq!(status.queued_item_count, 1);
    assert!(state.federation_aleph_bft_network_inbound_active().await);
    assert!(state.federation_aleph_bft_live_session_active().await);

    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_session_uses_recovered_session_for_network_pump_messages() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;

    let initial_quote_id = QuoteId::new();
    let initial_envelope = mint_quote_envelope(&state, &initial_quote_id);
    let mut adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        local_private.member_id,
        1,
        public_config.members.keys().copied(),
    )
    .expect("session one AlephBFT adapter");
    let recovered_session_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        1,
        0,
        vec![FederationConsensusItem::operation(initial_envelope)],
    )
    .expect("session one finalized unit");
    adapter
        .finalize_unit(recovered_session_unit)
        .expect("finalize recovered session unit");
    let entry = adapter.journal().entries()[0].clone();
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin session recovery transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist session one finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist session one journal entry");
    tx.commit().await.expect("commit session recovery state");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("recover session one runtime");
    state.federation.member_mut_for_test().consensus = consensus.expect("configured member handle");
    state.federation.member_mut_for_test().sequences = sequences.expect("configured member handle");
    assert_eq!(
        state
            .federation_aleph_bft_status()
            .await
            .expect("recovered AlephBFT status")
            .session,
        1
    );
    assert_eq!(
        federation_aleph_bft_live_session_startup_decision(&state)
            .await
            .expect("session one startup decision"),
        FederationAlephBftLiveSessionStartupDecision::Spawn {
            local_member: local_private.member_id,
            peer_count: 1,
            session_id: 1,
        }
    );

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 1),
    );
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let stale_pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &state,
        sync_config,
        stale_pump_config,
        transport.clone(),
    )
    .await
    .expect("start recovered session live session")
    .expect("live session handle");

    let next_envelope = mint_quote_envelope(&state, &QuoteId::new());
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_envelope(next_envelope)
        .expect("queue session one operation");
    time::timeout(
        Duration::from_secs(5),
        transport.wait_for_aleph_bft_network_message_request_matching(|_| true),
    )
    .await
    .expect("session one network-message request");
    let requests = transport.aleph_bft_network_message_requests();

    assert!(
        requests
            .iter()
            .all(|request| request.payload.session_id == 1),
        "live network pump must derive session from recovered consensus status"
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("session one network-message request verifies");
        assert_eq!(request.sender, local_private.member_id);
    }
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_end_to_end_stage_order() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");

    let latency = latency_reservation.install();
    let operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_latency_stage(
        &latency,
        operation_id,
        CdkAlephBftLatencyStage::NetworkMessageDelivered,
    )
    .await;
    wait_for_applied_operation(&session.first_state, operation_id).await;
    wait_for_applied_operation(&session.second_state, operation_id).await;

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    eprintln!(
        "AlephBFT latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_three_serial_operations() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");
    let latency = latency_reservation.install();
    let mut operation_ids = Vec::new();
    for _ in 0..3 {
        let operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
        wait_for_applied_operation(&session.first_state, operation_id).await;
        wait_for_applied_operation(&session.second_state, operation_id).await;
        operation_ids.push(operation_id);
    }

    for operation_id in operation_ids {
        let operation_events = latency.events_for_operation(operation_id);
        assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
        let end_to_end = latency_stage_delta(
            &operation_events,
            CdkAlephBftLatencyStage::SubmitQueued,
            CdkAlephBftLatencyStage::WaiterReturned,
        )
        .expect("submit-to-waiter latency");
        assert!(
            end_to_end < Duration::from_secs(5),
            "serial operation {operation_id} took {end_to_end:?}: {}",
            format_latency_events(&operation_events)
        );
        eprintln!(
            "AlephBFT serial latency probe for {operation_id}: {}",
            format_latency_events(&operation_events)
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_duplicate_fanout_joins_one_operation() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start live session");
    let latency = latency_reservation.install();
    let operation_id = submit_duplicate_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, operation_id).await;
    wait_for_applied_operation(&session.second_state, operation_id).await;

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    assert_eq!(
        latency_stage_count(&operation_events, CdkAlephBftLatencyStage::SubmitQueued),
        1,
        "duplicate fanout must queue one local AlephBFT operation: {}",
        format_latency_events(&operation_events)
    );
    assert_eq!(
        latency_stage_count(
            &operation_events,
            CdkAlephBftLatencyStage::DataProviderYielded
        ),
        1,
        "duplicate fanout must produce one CDK data payload: {}",
        format_latency_events(&operation_events)
    );
    assert_eq!(
        latency_stage_count(&operation_events, CdkAlephBftLatencyStage::WaiterReturned),
        2,
        "duplicate fanout must return both waiters: {}",
        format_latency_events(&operation_events)
    );
    let stats = session
        .first_state
        .federation_operation_notification_stats();
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
    assert_eq!(stats.duplicate_submissions, 1);
    eprintln!(
        "AlephBFT duplicate fanout latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_five_member_bft_operation() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let session = start_five_member_aleph_bft_live_session(fast_aleph_bft_live_session_options(
        pump_config,
        500,
    ))
    .await
    .expect("start five-member live session");
    let latency = latency_reservation.install();
    let operation_id = submit_latency_probe_mint_quote(
        session
            .states
            .first()
            .expect("five-member latency probe has local state"),
    )
    .await;
    for state in &session.states {
        wait_for_applied_operation(state, operation_id).await;
    }

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    let end_to_end = latency_stage_delta(
        &operation_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("submit-to-waiter latency");
    assert!(
        end_to_end < Duration::from_secs(5),
        "five-member operation {operation_id} took {end_to_end:?}: {}",
        format_latency_events(&operation_events)
    );
    eprintln!(
        "AlephBFT five-member latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_seven_member_bft_operation() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 512,
        max_peer_deliveries_per_message: 512,
    };
    let session = start_multi_member_aleph_bft_live_session(
        7,
        5,
        fast_aleph_bft_live_session_options(pump_config, 500),
    )
    .await
    .expect("start seven-member live session");
    let latency = latency_reservation.install();
    let operation_id = submit_latency_probe_mint_quote(
        session
            .states
            .first()
            .expect("seven-member latency probe has local state"),
    )
    .await;
    for state in &session.states {
        wait_for_applied_operation(state, operation_id).await;
    }

    let operation_events = latency.events_for_operation(operation_id);
    assert_latency_stage_order(&operation_events, LATENCY_EXPECTED_STAGES);
    let end_to_end = latency_stage_delta(
        &operation_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("submit-to-waiter latency");
    assert!(
        end_to_end < Duration::from_secs(10),
        "seven-member operation {operation_id} took {end_to_end:?}: {}",
        format_latency_events(&operation_events)
    );
    eprintln!(
        "AlephBFT seven-member latency probe for {operation_id}: {}",
        format_latency_events(&operation_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_session_latency_probe_records_default_cadence_follow_up() {
    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };
    let session = start_two_member_aleph_bft_live_session(
        FederationAlephBftLiveSessionOptions::new(pump_config).with_max_round(500),
    )
    .await
    .expect("start default-cadence live session");
    time::sleep(Duration::from_secs(3)).await;
    let latency = latency_reservation.install();

    let cold_operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, cold_operation_id).await;
    wait_for_applied_operation(&session.second_state, cold_operation_id).await;
    let warm_operation_id = submit_latency_probe_mint_quote(&session.first_state).await;
    wait_for_applied_operation(&session.first_state, warm_operation_id).await;
    wait_for_applied_operation(&session.second_state, warm_operation_id).await;

    let cold_events = latency.events_for_operation(cold_operation_id);
    let warm_events = latency.events_for_operation(warm_operation_id);
    assert_latency_stage_order(&cold_events, LATENCY_EXPECTED_STAGES);
    assert_latency_stage_order(&warm_events, LATENCY_EXPECTED_STAGES);
    let cold_end_to_end = latency_stage_delta(
        &cold_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("cold submit-to-waiter latency");
    let warm_end_to_end = latency_stage_delta(
        &warm_events,
        CdkAlephBftLatencyStage::SubmitQueued,
        CdkAlephBftLatencyStage::WaiterReturned,
    )
    .expect("warm submit-to-waiter latency");
    assert!(
        cold_end_to_end < Duration::from_secs(10),
        "default-cadence cold operation {cold_operation_id} took {cold_end_to_end:?}: {}",
        format_latency_events(&cold_events)
    );
    assert!(
        warm_end_to_end < Duration::from_secs(10),
        "default-cadence warm operation {warm_operation_id} took {warm_end_to_end:?}: {}",
        format_latency_events(&warm_events)
    );
    eprintln!(
        "AlephBFT default-cadence cold latency probe for {cold_operation_id}: {}",
        format_latency_events(&cold_events)
    );
    eprintln!(
        "AlephBFT default-cadence warm latency probe for {warm_operation_id}: {}",
        format_latency_events(&warm_events)
    );
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_operation_over_private_rest_network_messages() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    assert_eq!(
        first_state
            .federation_aleph_bft_status()
            .await
            .expect("first AlephBFT status")
            .finalized_item_count,
        1
    );
    assert_eq!(
        second_state
            .federation_aleph_bft_status()
            .await
            .expect("second AlephBFT status")
            .finalized_item_count,
        1
    );
    assert!(first_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("first quote lookup")
        .is_some());
    assert!(second_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("second quote lookup")
        .is_some());
}

#[tokio::test]
async fn aleph_bft_four_live_sessions_finalize_operation_over_private_rest_network_messages() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
    ];
    let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        threshold_params,
        federation_api_urls.clone(),
    );

    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
            )
            .await,
        );
    }

    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(30),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };

    let mut live_session_handles = Vec::new();
    for index in 0_usize..4 {
        let options = fast_aleph_bft_live_session_options(pump_config, 500);
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &states[index],
            sync_config,
            options,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 4);

    states[0].federation.member_mut_for_test().peer_sync_config = sync_config;

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("four-member live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
    }
}

#[tokio::test]
async fn aleph_bft_live_network_pump_wakes_on_outbound_before_peer_sync_interval() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_secs(60),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(20),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 1,
        max_peer_deliveries_per_message: 128,
    };
    first_state
        .federation
        .member_mut_for_test()
        .peer_sync_config = sync_config;

    let _first_handle = start_federation_aleph_bft_live_session_with_transport(
        &first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response without waiting for peer sync interval");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    assert!(transport
        .aleph_bft_network_message_requests()
        .iter()
        .any(|request| request.payload.session_id == 0));
}

#[tokio::test]
async fn aleph_bft_live_sessions_rotate_after_finality_through_router_clones() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_transport(
        &second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_response = first_state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("session zero operation response");
    assert_eq!(first_response.operation_id, first_operation_id);
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    assert_eq!(
        first_state
            .federation_aleph_bft_status()
            .await
            .expect("first session zero status")
            .session,
        0
    );
    assert_eq!(
        second_state
            .federation_aleph_bft_status()
            .await
            .expect("second session zero status")
            .finalized_item_count,
        1
    );

    let rotated_first_handle =
        rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
            &mut first_state,
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("rotate first live session")
        .expect("rotated first live session handle");
    let rotated_second_handle =
        rotate_federation_aleph_bft_live_session_to_next_empty_with_transport(
            &mut second_state,
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("rotate second live session")
        .expect("rotated second live session handle");

    assert!(!Arc::ptr_eq(&first_handle, &rotated_first_handle));
    assert!(!Arc::ptr_eq(&second_handle, &rotated_second_handle));
    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("rotated AlephBFT status");
        assert_eq!(status.session, 1);
        assert_eq!(status.finalized_item_count, 1);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_aleph_bft_active_session()
                .await
                .expect("rotated active session"),
            Some(1)
        );
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let second_response = first_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("session one operation response");
    assert_eq!(second_response.operation_id, second_operation_id);
    wait_for_applied_operation(&first_state, second_operation_id).await;
    wait_for_applied_operation(&second_state, second_operation_id).await;

    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("post-rotation AlephBFT status");
        assert_eq!(status.session, 1);
        assert_eq!(status.finalized_item_count, 2);
        assert_eq!(status.accepted_operation_count, 2);
        assert_eq!(status.next_unit_finalization_index, 1);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&second_quote_id)
            .await
            .expect("rotated quote lookup")
            .is_some());
    }

    drop(first_handle);
    drop(second_handle);
    drop(rotated_first_handle);
    drop(rotated_second_handle);
}

#[tokio::test]
async fn aleph_bft_live_sessions_auto_rotate_after_session_completion() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        second_private,
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(5),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let options =
        fast_aleph_bft_live_session_options(pump_config, 6).with_automatic_rotation_limit(2);

    let first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &first_state,
        sync_config,
        options.clone(),
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &second_state,
        sync_config,
        options,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let rotation_activity = FederationOperationId::from_bytes([202; 32]);
    time::timeout(Duration::from_secs(10), async {
        loop {
            let first_session = first_state
                .federation_aleph_bft_status()
                .await
                .expect("first auto-rotation status")
                .session;
            let second_session = second_state
                .federation_aleph_bft_status()
                .await
                .expect("second auto-rotation status")
                .session;
            if first_session >= 2 && second_session >= 2 {
                return;
            }

            // Empty sessions rotate only while bounded peer activity is present.
            for state in [&first_state, &second_state] {
                if let Some(handle) = state.federation.aleph_bft_live_session.read().await.clone() {
                    handle.mark_peer_activity(rotation_activity);
                }
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("active empty sessions did not auto-rotate before timeout");

    wait_for_aleph_bft_session(&first_state, 2).await;
    wait_for_aleph_bft_session(&second_state, 2).await;
    for state in [&first_state, &second_state] {
        time::timeout(Duration::from_secs(10), async {
            loop {
                if state.federation_aleph_bft_live_session_active().await
                    && state.federation_aleph_bft_network_inbound_active().await
                {
                    return;
                }
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("auto-rotated live session did not become active before timeout");
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_aleph_bft_active_session()
                .await
                .expect("auto-rotated active session"),
            Some(2)
        );
    }

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("session-one operation response");
    assert_eq!(response.operation_id, operation_id);
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;

    for state in [&first_state, &second_state] {
        let status = state
            .federation_aleph_bft_status()
            .await
            .expect("post-auto-rotation AlephBFT status");
        assert_eq!(status.session, 2);
        assert_eq!(status.accepted_operation_count, 1);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("auto-rotated quote lookup")
            .is_some());
    }

    drop(first_handle);
    drop(second_handle);
}
