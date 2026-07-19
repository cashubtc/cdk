#[tokio::test]
async fn unusable_stored_aleph_bft_backup_does_not_pin_startup_with_valid_peer() {
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
        first_private.clone(),
        first_db.clone(),
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private.clone(),
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
    let live_options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    );
    let _first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut first_state,
        sync_config.clone(),
        live_options.clone(),
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
    let _second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut second_state,
        sync_config.clone(),
        live_options.clone(),
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

    // Both members finalize one operation so their session backups are
    // nonempty and their operation counts are equal (no catch-up needed).
    let envelope = mint_quote_envelope(&second_state, &QuoteId::new());
    let operation_id = envelope.operation_id().expect("operation id");
    submit_live_operation_allowing_pending_timeout(&second_state, envelope).await;
    assert_eq!(
        wait_for_terminal_operation(&first_state, operation_id).await,
        FederationOperationStatus::Applied
    );
    let first_backup = first_db
        .get_federation_aleph_bft_backup_chunks(0)
        .await
        .expect("first backup lookup");
    assert!(
        !first_backup.is_empty(),
        "first member must hold session backup bytes before corruption"
    );

    // A previous unusable fetch has left semantically invalid bytes in the
    // durable backup. The member does not need catch-up, so the ordinary
    // backup path will not replace them.
    stop_current_federation_aleph_bft_session_for_catch_up(&first_state).await;
    {
        let mut tx = first_db.begin_transaction().await.expect("begin corrupt");
        tx.replace_federation_aleph_bft_backup_chunks(0, &[vec![0xFF; 1024]])
            .await
            .expect("write unusable backup bytes");
        tx.commit().await.expect("commit unusable backup bytes");
    }
    let _restarted_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut first_state,
        sync_config,
        live_options,
        transport.clone(),
    )
    .await
    .expect("restart first live session with unusable stored backup");

    // Wait for the early-exit teardown: the generation is stopped and the
    // unusable bytes are quarantined before peer sync recovers.
    time::timeout(Duration::from_secs(10), async {
        loop {
            let backup_chunks = first_db
                .get_federation_aleph_bft_backup_chunks(0)
                .await
                .map(|chunks| chunks.len())
                .unwrap_or(0);
            if !first_state.federation_aleph_bft_live_session_active().await && backup_chunks == 0 {
                break;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("unusable backup must be quarantined and the generation stopped");

    // The healthy second member serves a valid backup for the same session.
    // Peer sync must fetch it and restart the same session instead of
    // rotating away and splitting the federation.
    let client = FederationRestPeerClient::new(transport, &public_config, first_private)
        .expect("first member peer client");
    let second_member = public_config
        .members
        .values()
        .find(|member| member.member_id == second_private.member_id)
        .expect("second member public config")
        .clone();
    time::timeout(Duration::from_secs(20), async {
        loop {
            run_federation_peer_sync_round(
                &first_state,
                &client,
                std::slice::from_ref(&second_member),
            )
            .await;
            let backup_chunks = first_db
                .get_federation_aleph_bft_backup_chunks(0)
                .await
                .map(|chunks| chunks.len())
                .unwrap_or(0);
            if first_state.federation_aleph_bft_live_session_active().await
                && matches!(
                    first_state.federation_sync_status().await,
                    Some(crate::FederationSyncStatus::Healthy)
                )
                && backup_chunks > 0
            {
                break;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("peer sync must restore the same session from a valid backup");

    // Recovery is live, not cosmetic: a fresh operation finalizes on both
    // members after the restart.
    let recovered_envelope = mint_quote_envelope(&second_state, &QuoteId::new());
    let recovered_operation_id = recovered_envelope.operation_id().expect("operation id");
    let submit_state = second_state.clone();
    let submit_task = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(recovered_envelope)
            .await
    });
    assert_eq!(
        wait_for_terminal_operation(&first_state, recovered_operation_id).await,
        FederationOperationStatus::Applied
    );
    assert_eq!(
        wait_for_terminal_operation(&second_state, recovered_operation_id).await,
        FederationOperationStatus::Applied
    );
    let _ = submit_task.abort();
    let _ = submit_task.await;
}

struct MultiMemberAlephBftLiveSession {
    states: Vec<MintState>,
    dbs: Vec<Arc<cdk_sqlite::MintSqliteDatabase>>,
    seeds: Vec<Vec<u8>>,
    transport: RouterFederationTransportRegistry,
    federation_api_urls: Vec<MintUrl>,
    public_config: FederationConfig,
    private_configs: Vec<MemberConfigPrivate>,
    sync_config: FederationPeerSyncConfig,
    _handles: Vec<Arc<AlephBftSessionSupervisor>>,
}

async fn start_two_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<TwoMemberAlephBftLiveSession, cdk::Error> {
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

    let first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut first_state,
        sync_config,
        live_options.clone(),
        transport.clone(),
    )
    .await?
    .expect("first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let second_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &mut second_state,
        sync_config,
        live_options,
        transport.clone(),
    )
    .await?
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    Ok(TwoMemberAlephBftLiveSession {
        first_state,
        second_state,
        _first_handle: first_handle,
        _second_handle: second_handle,
    })
}

async fn start_five_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    start_multi_member_aleph_bft_live_session(5, 3, live_options).await
}

async fn start_multi_member_aleph_bft_live_session(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    let (session, _) = start_multi_member_aleph_bft_live_session_with_payment_policy(
        member_count,
        signature_threshold,
        live_options,
        FederationPaymentObservationPolicy::development_single_observation_for_testing(),
    )
    .await?;

    Ok(session)
}

async fn start_multi_member_aleph_bft_live_session_with_payment_policy(
    member_count: u16,
    signature_threshold: u16,
    live_options: FederationAlephBftLiveSessionOptions,
    payment_observation_policy: FederationPaymentObservationPolicy,
) -> Result<(MultiMemberAlephBftLiveSession, Vec<Arc<AtomicUsize>>), cdk::Error> {
    let federation_api_urls = (1..=member_count)
        .map(|value| {
            MintUrl::from_str(&format!("http://fed{value}.test")).expect("federation api url")
        })
        .collect::<Vec<_>>();
    let (mut public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(member_count, signature_threshold)
            .expect("threshold params"),
        federation_api_urls.clone(),
    );
    public_config.payment_observation_policy = payment_observation_policy;
    public_config.validate()?;
    let mut states = Vec::new();
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut make_payment_calls = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let (payment_processor, member_make_payment_calls) =
            CountingFakeWallet::with_call_counter();
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft_payment_processor(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
                payment_processor,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed.to_vec());
        make_payment_calls.push(member_make_payment_calls);
    }
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let mut handles = Vec::new();
    for (index, state) in states.iter_mut().enumerate() {
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            state,
            sync_config,
            live_options.clone(),
            transport.clone(),
        )
        .await?
        .expect("live session handle");
        handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
    }

    Ok((
        MultiMemberAlephBftLiveSession {
            states,
            dbs,
            seeds,
            transport,
            federation_api_urls,
            public_config,
            private_configs,
            sync_config,
            _handles: handles,
        },
        make_payment_calls,
    ))
}

async fn assert_multi_member_journal_and_state_converged(
    session: &MultiMemberAlephBftLiveSession,
    expected_operation_ids: &[FederationOperationId],
) {
    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected converged journal");
    assert_eq!(
        expected_entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>(),
        expected_operation_ids
    );
    let expected_projection = journal_convergence_projection(&expected_entries);
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected converged materialized-state digest");
    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged journal");
        assert_eq!(
            journal_convergence_projection(&entries),
            expected_projection
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged materialized-state digest"),
            expected_digest
        );
    }
}

async fn run_five_member_restart_case(restarted_indexes: &[usize]) {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 500);
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("five-member restart session");

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&session.states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("operation before restart");
    for state in &session.states {
        wait_for_applied_operation(state, first_operation_id).await;
    }
    for &index in restarted_indexes {
        wait_for_aleph_bft_backup_bytes(&session.states[index], 0).await;
        *session.states[index]
            .federation
            .aleph_bft_network_inbound
            .write()
            .await = None;
        *session.states[index]
            .federation
            .aleph_bft_live_session
            .write()
            .await = None;
        session
            .transport
            .remove(&session.federation_api_urls[index]);
    }
    for &index in restarted_indexes.iter().rev() {
        drop(session._handles.remove(index));
    }

    for &index in restarted_indexes {
        let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
            session.public_config.clone(),
            session.private_configs[index].clone(),
            Arc::clone(&session.dbs[index]),
            &session.seeds[index],
        )
        .await;
        replay_aleph_bft_runtime_after_test_process_restart(&mut state).await;
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &state,
            session.sync_config,
            live_options.clone(),
            session.transport.clone(),
        )
        .await
        .expect("restart live member")
        .expect("restarted live member handle");
        session.transport.insert(
            session.federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(state.clone()),
        );
        session.states[index] = state;
        session._handles.push(handle);
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&session.states[0], &second_quote_id);
    let second_operation_id = second_envelope
        .operation_id()
        .expect("post-restart operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("operation after restart");
    for state in &session.states {
        wait_for_applied_operation(state, second_operation_id).await;
    }
    assert_multi_member_journal_and_state_converged(
        &session,
        &[first_operation_id, second_operation_id],
    )
    .await;
}

