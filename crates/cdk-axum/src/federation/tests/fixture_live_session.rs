//! Multi-member live AlephBFT sessions and apply helpers.

use super::*;

pub(super) struct TwoMemberAlephBftLiveSession {
    pub(super) first_state: MintState,
    pub(super) second_state: MintState,
    pub(super) _first_handle: Arc<AlephBftSessionSupervisor>,
    pub(super) _second_handle: Arc<AlephBftSessionSupervisor>,
}

pub(super) struct MultiMemberAlephBftLiveSession {
    pub(super) states: Vec<MintState>,
    pub(super) dbs: Vec<Arc<cdk_sqlite::MintSqliteDatabase>>,
    pub(super) seeds: Vec<Vec<u8>>,
    pub(super) transport: RouterFederationTransportRegistry,
    pub(super) federation_api_urls: Vec<MintUrl>,
    pub(super) public_config: FederationConfig,
    pub(super) private_configs: Vec<MemberConfigPrivate>,
    pub(super) sync_config: FederationPeerSyncConfig,
    pub(super) _handles: Vec<Arc<AlephBftSessionSupervisor>>,
}

pub(super) async fn start_two_member_aleph_bft_live_session(
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
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private,
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
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
        &first_state,
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
        &second_state,
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

pub(super) async fn start_five_member_aleph_bft_live_session(
    live_options: FederationAlephBftLiveSessionOptions,
) -> Result<MultiMemberAlephBftLiveSession, cdk::Error> {
    start_multi_member_aleph_bft_live_session(5, 3, live_options).await
}

pub(super) async fn start_multi_member_aleph_bft_live_session(
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

pub(super) async fn start_multi_member_aleph_bft_live_session_with_payment_policy(
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

pub(super) async fn assert_multi_member_journal_and_state_converged(
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

pub(super) async fn run_five_member_restart_case(restarted_indexes: &[usize]) {
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

pub(super) struct ApplyRetryBudgetOverrideGuard;

impl ApplyRetryBudgetOverrideGuard {
    pub(super) fn set(millis: u64) -> Self {
        crate::FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS
            .store(millis, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for ApplyRetryBudgetOverrideGuard {
    fn drop(&mut self) {
        crate::FEDERATION_APPLY_RETRY_BUDGET_OVERRIDE_MILLIS
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Test-scale inbound channel budget used by the backpressure regression so
/// the flood stays small and deterministic.
pub(super) const FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_TEST: u64 = 128;

pub(super) async fn apply_identical_finalized_unit(
    first: &MintState,
    second: &MintState,
    items: Vec<FederationConsensusItem>,
) {
    let payload = {
        let consensus = first
            .federation
            .consensus()
            .expect("first agreement consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            items,
        )
        .expect("agreement payload")
    };

    first
        .federation
        .apply_finalized_unit(payload.clone())
        .await
        .expect("apply first agreement unit");
    second
        .federation
        .apply_finalized_unit(payload)
        .await
        .expect("apply second agreement unit");
}
