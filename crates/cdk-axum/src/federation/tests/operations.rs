#[tokio::test]
async fn aleph_bft_public_submit_is_not_ready_until_live_session_is_started() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("AlephBFT public submit should be gated until a live session is started");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeAwaitingQuorum { .. })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_public_submit_stays_gated_when_sync_is_healthy_without_live_session() {
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
    state.mark_federation_healthy().await;
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("healthy sync must not reopen AlephBFT submit without a live session");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::RuntimeUnavailable {
            component: "AlephBFT live session",
        })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_live_session_opens_queueing_but_still_waits_for_finalized_unit() {
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
    state.mark_federation_healthy().await;
    let sync_config = FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    };
    state.federation.peer_sync_config = Some(sync_config);
    assert!(matches!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));
    assert!(!state.federation_aleph_bft_live_session_active().await);
    assert!(!state.federation_aleph_bft_network_inbound_active().await);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect_err("healthy sync without a live AlephBFT session must stay gated");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::RuntimeUnavailable {
            component: "AlephBFT live session",
        })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after gated submit")
        .is_none());
    {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &mut state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        transport,
    )
    .await
    .expect("start AlephBFT live session")
    .expect("live session handle");
    assert!(state.federation_aleph_bft_live_session_active().await);
    assert!(state.federation_aleph_bft_network_inbound_active().await);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("live AlephBFT session should still wait for a finalized unit");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after unfinalized submit")
        .is_none());
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert_eq!(consensus.peer_status().operation_count, 0);
    drop(handle);
}

#[tokio::test]
async fn aleph_bft_network_pump_broadcasts_everyone_to_remote_members() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(42, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        network
            .next_event()
            .await
            .expect("broadcast loops back to local AlephBFT network"),
        42
    );
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert!(!outcome.reached_idle);
    let requests = transport.aleph_bft_network_message_requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.receiver)
            .collect::<Vec<_>>(),
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    for request in requests {
        request
            .verify(&public_config, request.receiver)
            .expect("signed request verifies");
        assert_eq!(request.sender, local_private.member_id);
        assert_eq!(request.payload.session_id, 7);
        let bytes = request.payload.message_bytes().expect("message bytes");
        let decoded = u64::decode(&mut &bytes[..]).expect("scale u64");
        assert_eq!(decoded, 42);
    }
}

#[tokio::test]
async fn aleph_bft_network_pump_delivers_already_awaited_outbound_message() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(42, Recipient::Everyone);
    let first_outbound = handle.next_outgoing().await.expect("outbound message");
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
        Some(first_outbound),
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        network
            .next_event()
            .await
            .expect("already-awaited broadcast loops back locally"),
        42
    );
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_pump_leaves_retry_queue_empty_after_success() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(42, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 7,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        outcome.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_requeued_members.is_empty());
    assert_eq!(outcome.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
}

#[tokio::test]
async fn aleph_bft_network_pump_quorum_fast_path_does_not_block_fresh_messages_on_slow_peer() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_network_message_response(response)
        .with_mock_aleph_bft_network_message_delays([
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(5),
        ]);
    let client = FederationRestPeerClient::new_with_config(
        transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            peer_request_timeout: Duration::from_secs(10),
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 7,
        ..CdkAlephBftNetworkPumpConfig::default()
    };

    network.send(41, Recipient::Everyone);
    let first = time::timeout(
        Duration::from_secs(1),
        run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
            &mut handle,
            &keychain,
            &client,
            pump_config,
            &mut retry_queue,
        ),
    )
    .await
    .expect("quorum delivery must not await the slow peer timeout")
    .expect("first pump tick");
    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert_eq!(first.remote_accepted_members.len(), 3);
    assert!(first.remote_failed_members.is_empty());
    assert_eq!(first.remote_requeued_members.len(), 1);
    assert_eq!(first.retry_queue_len, 1);

    network.send(42, Recipient::Everyone);
    let second = time::timeout(
        Duration::from_secs(1),
        run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
            &mut handle,
            &keychain,
            &client,
            pump_config,
            &mut retry_queue,
        ),
    )
    .await
    .expect("fresh message must bypass queued retry work")
    .expect("second pump tick");
    assert_eq!(second.messages_drained, 1);
    assert_eq!(second.retry_messages_drained, 0);
    assert_eq!(second.remote_accepted_members.len(), 4);
    assert_eq!(second.retry_queue_len, 1);

    let retry = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("idle retry tick");
    assert_eq!(retry.messages_drained, 0);
    assert_eq!(retry.retry_messages_drained, 1);
    assert_eq!(retry.remote_accepted_members.len(), 1);
    assert_eq!(retry.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
}

#[tokio::test]
async fn aleph_bft_network_pump_dispatches_slow_message_batch_concurrently() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 7);
    let transport = MockFederationTransport::default()
        .with_aleph_bft_network_message_response(response)
        .with_mock_aleph_bft_network_message_delays([
            Duration::from_millis(250),
            Duration::from_millis(250),
            Duration::from_millis(250),
            Duration::from_millis(250),
        ]);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    for payload in 0..4 {
        network.send(payload, Recipient::Node(NodeIndex(1)));
    }
    let outcome = time::timeout(
        Duration::from_millis(750),
        run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
            &mut handle,
            &keychain,
            &client,
            CdkAlephBftNetworkPumpConfig {
                session_id: 7,
                max_messages_per_tick: 4,
                ..CdkAlephBftNetworkPumpConfig::default()
            },
            &mut retry_queue,
        ),
    )
    .await
    .expect("slow outbound messages should run as one concurrent batch")
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 4);
    assert_eq!(outcome.remote_accepted_members.len(), 4);
    assert!(outcome.remote_failed_members.is_empty());
    assert!(retry_queue.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_pump_sends_node_recipient_to_one_remote_member() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 8);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(99, Recipient::Node(NodeIndex(2)));
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 8,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(outcome.remote_accepted_members, vec![member_id(3)]);
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    let requests = transport.aleph_bft_network_message_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].receiver, member_id(3));
    let bytes = requests[0].payload.message_bytes().expect("message bytes");
    let decoded = u64::decode(&mut &bytes[..]).expect("scale u64");
    assert_eq!(decoded, 99);
}

#[tokio::test]
async fn aleph_bft_network_pump_does_not_rest_send_local_node_recipient() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 9);
    let transport =
        MockFederationTransport::default().with_aleph_bft_network_message_response(response);
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (mut network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(100, Recipient::Node(NodeIndex(0)));
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 9,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(
        network
            .next_event()
            .await
            .expect("local node recipient loops back locally"),
        100
    );
    assert!(outcome.remote_accepted_members.is_empty());
    assert!(outcome.remote_failed_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert!(transport.aleph_bft_network_message_requests().is_empty());
}

#[tokio::test]
async fn aleph_bft_network_pump_records_delivery_failures_without_failing_tick() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();

    network.send(123, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 10,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
    )
    .await
    .expect("network pump tick records peer failures");

    assert_eq!(outcome.messages_drained, 1);
    assert!(outcome.remote_accepted_members.is_empty());
    assert_eq!(
        outcome.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_redelivers_failed_members_after_recovery() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(456, Recipient::Everyone);
    let first = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 11,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump first tick");

    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert!(first.remote_accepted_members.is_empty());
    assert_eq!(
        first.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert_eq!(first.remote_requeued_members, first.remote_failed_members);
    assert_eq!(first.retry_queue_len, 1);
    assert_eq!(retry_queue.len(), 1);
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);

    let response = FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 11);
    transport
        .clone()
        .with_aleph_bft_network_message_response(response);
    let second = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 11,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump retry tick");

    assert_eq!(second.messages_drained, 0);
    assert_eq!(second.retry_messages_drained, 1);
    assert_eq!(
        second.remote_accepted_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(second.remote_failed_members.is_empty());
    assert!(second.remote_requeued_members.is_empty());
    assert_eq!(second.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 8);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_retries_requeued_failure_once_per_tick() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 12,
        max_messages_per_tick: 8,
        max_peer_deliveries_per_message: 8,
    };

    network.send(789, Recipient::Everyone);
    let first = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("network pump first tick");
    let first_request_count = transport.aleph_bft_network_message_requests().len();

    assert_eq!(first.messages_drained, 1);
    assert_eq!(first.retry_messages_drained, 0);
    assert_eq!(first_request_count, 4);
    assert_eq!(retry_queue.len(), 1);

    let second = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        pump_config,
        &mut retry_queue,
    )
    .await
    .expect("network pump retry tick");

    assert_eq!(second.messages_drained, 0);
    assert_eq!(second.retry_messages_drained, 1);
    assert_eq!(second.retry_queue_len, 1);
    assert_eq!(retry_queue.len(), 1);
    assert_eq!(
        transport.aleph_bft_network_message_requests().len(),
        first_request_count + 4
    );
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_drops_non_retryable_status_failures() {
    let (public_config, private_configs) = transport_config_five_with_threshold(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
    );
    let local_private = private_configs[0].clone();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_status_error(
        400,
        "Invalid federation AlephBFT network message: Codec error",
    );
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair();
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    network.send(789, Recipient::Everyone);
    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 12,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("network pump tick handles non-retryable peer failures");

    assert_eq!(outcome.messages_drained, 1);
    assert_eq!(outcome.retry_messages_drained, 0);
    assert!(outcome.remote_accepted_members.is_empty());
    assert_eq!(
        outcome.remote_failed_members,
        vec![member_id(2), member_id(3), member_id(4), member_id(5)]
    );
    assert!(outcome.remote_requeued_members.is_empty());
    assert!(outcome.remote_skipped_members.is_empty());
    assert_eq!(outcome.retry_queue_len, 0);
    assert!(retry_queue.is_empty());
    assert_eq!(transport.aleph_bft_network_message_requests().len(), 4);
}

#[tokio::test]
async fn aleph_bft_network_retry_queue_evicts_oldest_delivery_at_bound() {
    let (public_config, local_private, _) = transport_config();
    let keychain =
        CdkAlephBftKeychain::try_new(&public_config, local_private.clone()).expect("keychain");
    let transport = MockFederationTransport::default();
    let client =
        FederationRestPeerClient::new(transport.clone(), &public_config, local_private.clone())
            .expect("peer client");
    // This test exercises the retry-queue bound, not the bounded session
    // channel, so give the pair enough capacity to hold every message.
    let message_count = CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT + 1;
    let (network, mut handle) = CdkAlephBftNetwork::<u64>::new_pair_with_capacity(message_count);
    let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();

    for payload in 0..message_count as u64 {
        network.send(payload, Recipient::Everyone);
    }

    let outcome = run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
        &mut handle,
        &keychain,
        &client,
        CdkAlephBftNetworkPumpConfig {
            session_id: 13,
            max_messages_per_tick: message_count,
            ..CdkAlephBftNetworkPumpConfig::default()
        },
        &mut retry_queue,
    )
    .await
    .expect("overflowing network pump tick");

    assert_eq!(outcome.messages_drained, message_count);
    assert_eq!(outcome.retry_messages_drained, 0);
    assert_eq!(outcome.remote_failed_members.len(), message_count);
    assert!(outcome
        .remote_failed_members
        .iter()
        .all(|failed_member_id| *failed_member_id == member_id(2)));
    assert_eq!(outcome.remote_requeued_members.len(), message_count);
    assert_eq!(
        outcome.retry_queue_len,
        CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT
    );
    assert_eq!(retry_queue.len(), CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT);
    assert_eq!(outcome.retry_deliveries_dropped, 1);
    assert_eq!(
        transport.aleph_bft_network_message_requests().len(),
        message_count
    );
}

#[tokio::test]
async fn aleph_bft_startup_does_not_restore_transient_healthy_status() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    first_state.mark_federation_healthy().await;
    assert!(matches!(
        first_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    ));

    let restored_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    assert!(matches!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::AwaitingQuorum { operation_count: 0 })
    ));

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&restored_state, &quote_id);
    let err = restored_state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("transient healthy status must not reopen AlephBFT public submit");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeAwaitingQuorum { operation_count: 0 })
    ));
    assert!(restored_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restored gated submit")
        .is_none());

    let consensus = restored_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
}

#[tokio::test]
async fn aleph_bft_startup_restore_requires_quorum_even_with_manual_live_session() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let restored_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    assert!(matches!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::AwaitingQuorum { operation_count: 0 })
    ));

    let sync_config = FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    };
    let transport = MockFederationTransport::default().with_aleph_bft_network_message_response(
        FederationAlephBftNetworkMessageResponse::new(public_config.federation_id, 0),
    );
    let handle = start_federation_aleph_bft_live_session_with_transport(
        &restored_state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
        transport,
    )
    .await
    .expect("start AlephBFT live session")
    .expect("live session handle");
    restored_state.restore_federation_startup_readiness().await;
    assert_eq!(
        restored_state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::AwaitingQuorum { operation_count: 0 })
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&restored_state, &quote_id);
    let err = restored_state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("live session without quorum validation must remain gated");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::NodeAwaitingQuorum { operation_count: 0 })
    ));
    assert!(restored_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after pending finality")
        .is_none());
    let consensus = restored_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 0);
    drop(handle);
}

#[tokio::test]
async fn quorum_reconciliation_marks_new_live_session_healthy() {
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
    let client = FederationRestPeerClient::new(
        RouterFederationTransportRegistry::default(),
        &public_config,
        local_private,
    )
    .expect("quorum reconciliation client");
    let mut peer_status = local_peer_status(&state).await.expect("local peer status");
    peer_status.member_id = peer_private.member_id;

    sync_loop::reconcile_federation_startup_with_quorum(
        &state,
        &client,
        tokio::time::Instant::now(),
        &[peer_status],
    )
    .await;

    assert_eq!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );
    assert!(state.federation.live_session_active().await);
    halt_current_federation_aleph_bft_session(&state, "test complete").await;
}

#[tokio::test]
async fn quorum_reconciliation_replaces_stale_live_session() {
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
    let transport = RouterFederationTransportRegistry::default();
    start_federation_aleph_bft_live_session_with_transport(
        &state,
        FederationPeerSyncConfig::default(),
        CdkAlephBftNetworkPumpConfig::default(),
        transport.clone(),
    )
    .await
    .expect("start stale live session")
    .expect("stale live session handle");
    state.restore_federation_startup_readiness().await;

    let client = FederationRestPeerClient::new(transport, &public_config, local_private)
        .expect("quorum reconciliation client");
    let mut peer_status = local_peer_status(&state).await.expect("local peer status");
    peer_status.member_id = peer_private.member_id;
    peer_status.aleph_bft_session = 1;
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_chunks(1)
        .await
        .expect("next-session backup lookup")
        .is_empty());

    sync_loop::reconcile_federation_startup_with_quorum(
        &state,
        &client,
        tokio::time::Instant::now(),
        &[peer_status],
    )
    .await;

    assert_eq!(
        state.federation_sync_status().await,
        Some(crate::FederationSyncStatus::Healthy)
    );
    assert_eq!(
        local_peer_status(&state)
            .await
            .expect("reconciled local status")
            .aleph_bft_session,
        1
    );
    assert!(state.federation.live_session_active().await);
    halt_current_federation_aleph_bft_session(&state, "test complete").await;
}

#[tokio::test]
async fn aleph_bft_restart_after_unfinalized_local_unit_requires_retry_without_state_mutation() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let seed = mnemonic.to_seed_normalized("");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db.clone(),
        &seed,
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope.clone()).await;
    assert_eq!(unit.operation_ids, vec![operation_id]);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before restart")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal before restart")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items before restart")
        .is_empty());
    {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.pending_mempool_len(), 1);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }
    drop(state);

    let restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &seed,
    )
    .await;
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restart")
        .is_none());
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after restart")
        .is_empty());
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after restart")
        .is_empty());
    {
        let consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("restarted consensus");
        let consensus = consensus.lock().await;
        assert_eq!(consensus.queued_len(), 0);
        assert_eq!(consensus.pending_mempool_len(), 0);
        assert_eq!(consensus.peer_status().operation_count, 0);
    }

    let retried_unit = queue_local_aleph_bft_unit(&restarted_state, envelope).await;
    assert_eq!(retried_unit.operation_ids, vec![operation_id]);
    assert_eq!(
        retried_unit.canonical_bytes().expect("retried unit bytes"),
        unit.canonical_bytes().expect("original unit bytes")
    );
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after retry queue")
        .is_none());
}

#[tokio::test]
async fn aleph_bft_submit_envelope_waits_for_finalization() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let submit_response = submit.await.expect("submit task");
    assert_eq!(submit_response.operation_id, operation_id);
    assert!(matches!(
        submit_response.outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));
    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after submit")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
}

#[tokio::test]
async fn aleph_bft_submit_envelope_does_not_persist_dkg_sequence_state() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect("AlephBFT submit response")
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    let sequences_before = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences before finalization");
    assert!(sequences_before.is_empty());

    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    submit.await.expect("submit task");
    let sequences_after = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequences after finalization");
    assert!(
        sequences_after.is_empty(),
        "operation finalization must not mutate durable DKG replay state"
    );
}

#[tokio::test]
async fn aleph_bft_submit_envelope_times_out_without_finalization() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unfinalized submit should time out");

    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after timeout")
        .is_none());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
}

#[tokio::test]
async fn aleph_bft_timed_out_submit_retry_receives_finalized_response_without_requeuing() {
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
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let err = state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("unfinalized submit should time out");
    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 2,
        ..FederationPeerSyncConfig::default()
    });
    let retry_state = state.clone();
    let retry_submit = tokio::spawn(async move {
        retry_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let retry_response = retry_submit
        .await
        .expect("retry submit task")
        .expect("retry submit response");
    assert_eq!(retry_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after finalization")
        .is_some());
}

#[tokio::test]
async fn aleph_bft_submit_envelope_releases_response_waiter_after_timeout() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let first_err = state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect_err("first unfinalized submit should time out");
    assert_pending_finalization_error(&first_err, first_operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let second_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let second_operation_id = second_envelope.operation_id().expect("second operation id");

    let second_err = state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect_err("second unfinalized submit should time out after waiter cleanup");
    assert_pending_finalization_error(&second_err, second_operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
}

#[tokio::test]
async fn aleph_bft_submit_envelope_rejects_when_response_waiters_are_full() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let rejected_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let rejected_operation_id = rejected_envelope
        .operation_id()
        .expect("rejected operation id");
    let err = state
        .submit_federation_envelope_to_consensus(rejected_envelope)
        .await
        .expect_err("full response waiter set should reject");
    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::ResponseWaiterLimitExceeded { waiting: 1, max: 1 })
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let session = consensus
        .aleph_bft_status()
        .expect("consensus status")
        .session;
    drop(consensus);
    let pending = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after waiter rejection");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, first_operation_id);
    assert!(pending
        .iter()
        .all(|operation| operation.operation_id != rejected_operation_id));

    first_submit.abort();
    let _ = first_submit.await;
    state
        .wait_for_federation_pending_response_waiter_count(0)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
}

#[tokio::test]
async fn aleph_bft_pending_persistence_failure_releases_waiter_and_allows_retry() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        response_waiter_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let envelope = mint_quote_envelope(&state, &QuoteId::new());
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .federation
        .fail_next_pending_operation_persistence_for_test();

    state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("injected pending persistence failure");

    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    let session = consensus
        .aleph_bft_status()
        .expect("consensus status")
        .session;
    assert_eq!(consensus.queued_len(), 0);
    drop(consensus);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after persistence failure")
        .is_empty());

    let retry_error = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unfinalized retry should time out after successful admission");
    assert_pending_finalization_error(&retry_error, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);
    let pending = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(session)
        .await
        .expect("pending operations after retry");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation_id, operation_id);
}

#[tokio::test]
async fn aleph_bft_finalization_persistence_failure_leaves_runtime_unchanged() {
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
    let payload = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let consensus = consensus.lock().await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            status.next_unit_index,
            vec![FederationConsensusItem::operation(mint_quote_envelope(
                &state,
                &QuoteId::new(),
            ))],
        )
        .expect("finalized unit")
    };
    let before = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .status();
    state
        .federation
        .fail_next_finalization_persistence_for_test();

    state
        .apply_federation_aleph_bft_finalized_unit(payload.clone())
        .await
        .expect_err("injected finalization persistence failure");

    let after = state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .status();
    assert_eq!(after, before);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items after failed persistence")
        .is_empty());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries_from(0)
        .await
        .expect("journal entries after failed persistence")
        .is_empty());

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(payload)
        .await
        .expect("retry finalized unit after persistence recovers");
    assert_eq!(responses.len(), 1);
}

#[cfg(feature = "federation-bdk")]
#[tokio::test]
async fn onchain_reconciliation_failure_does_not_halt_finalized_application() {
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

    // A local BDK/projection failure strikes the reconciliation step of the
    // next finalized-unit application on both members.
    session
        .first_state
        .federation
        .fail_next_onchain_reconciliation_for_test();
    session
        .second_state
        .federation
        .fail_next_onchain_reconciliation_for_test();

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&session.first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = session.first_state.clone();
    let submit_task = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    // Wait until the operation applies on the second member, or any member
    // halts at the reconciliation boundary first.
    let mut applied = false;
    let mut halt_reason = None;
    time::timeout(Duration::from_secs(20), async {
        loop {
            for state in [&session.first_state, &session.second_state] {
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
            if let Some(entry) = session
                .second_state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(operation_id)
                .await
                .expect("journal lookup")
            {
                if entry.status() == FederationOperationStatus::Applied {
                    applied = true;
                    break;
                }
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finalized unit reached a decision before timeout");

    assert!(
        halt_reason.is_none(),
        "a local BDK reconciliation failure halted a member: {}",
        halt_reason.as_deref().unwrap_or("unknown halt")
    );
    assert!(
        applied,
        "the accepted operation must apply despite the reconciliation failure"
    );

    for state in [&session.first_state, &session.second_state] {
        assert!(!matches!(
            state.federation_sync_status().await,
            Some(crate::FederationSyncStatus::Halted { .. })
        ));
        assert!(state.federation_aleph_bft_live_session_active().await);
        assert!(state.federation_aleph_bft_network_inbound_active().await);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(
            state
                .federation
                .bdk_degraded_reason_for_test()
                .await
                .is_some(),
            "member must report BDK-degraded after the reconciliation failure"
        );
    }

    // Replaying the runtime state (the restart path) must not fail and must
    // preserve the BDK-degraded state instead of halting the member again.
    let mut first_state = session.first_state.clone();
    replay_aleph_bft_runtime_after_test_process_restart(&mut first_state).await;
    assert!(
        first_state
            .federation
            .bdk_degraded_reason_for_test()
            .await
            .is_some(),
        "the BDK-degraded state must survive restart replay"
    );

    // A follow-up application reconciles healthily and clears the degraded
    // state.
    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&session.second_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("operation id");
    session
        .second_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second operation submit");
    for state in [&session.first_state, &session.second_state] {
        assert_eq!(
            wait_for_terminal_operation(state, second_operation_id).await,
            FederationOperationStatus::Applied
        );
        assert!(
            state
                .federation
                .bdk_degraded_reason_for_test()
                .await
                .is_none(),
            "a healthy reconciliation must clear the degraded state"
        );
    }

    let _ = submit_task.abort();
    let _ = submit_task.await;
}

struct ApplyRetryBudgetOverrideGuard;

impl ApplyRetryBudgetOverrideGuard {
    fn set(millis: u64) -> Self {
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
const FEDERATION_ALEPH_BFT_CHANNEL_CAPACITY_TEST: u64 = 128;

#[tokio::test]
async fn recovery_candidate_fetch_falls_back_from_withholding_first_source() {
    let (public_config, first_private, second_private, _) = transport_config_three();
    let first_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        first_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db")),
        &[9; 64],
    )
    .await;
    let second_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        second_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db")),
        &[8; 64],
    )
    .await;
    let recovering_state = federation_route_state_for_private_with_db(
        public_config.clone(),
        second_private.clone(),
        Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("recovering db"),
        ),
        &[7; 64],
    )
    .await;

    // Member 1 finalizes one operation; every member applies the identical
    // finalized unit so all of them hold the same durable history.
    let history_envelope = mint_quote_envelope(&first_state, &QuoteId::new());
    let history_payload = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        member_id(1),
        0,
        0,
        vec![FederationConsensusItem::operation(history_envelope)],
    )
    .expect("history unit");
    for state in [&first_state, &second_state, &recovering_state] {
        state
            .apply_federation_aleph_bft_finalized_unit(history_payload.clone())
            .await
            .expect("apply shared finalized unit");
    }

    // Every advertiser holds the same recovery checkpoint candidate.
    let (checkpoint, _quorum) = recovery_checkpoint_quorum_for_state(
        &first_state,
        &public_config,
        None,
        &[&first_private, &second_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("checkpoint id");
    let first_member = public_config.member(member_id(1)).expect("first member");
    let second_member = public_config.member(member_id(2)).expect("second member");
    for state in [&first_state, &second_state] {
        state
            .federation
            .persist_checkpoint_candidate(&checkpoint)
            .await
            .expect("persist advertised recovery candidate");
    }

    let first_summary = first_state
        .federation
        .checkpoint_summary(Some(0))
        .await
        .expect("first advertiser summary");
    let second_summary = second_state
        .federation
        .checkpoint_summary(Some(0))
        .await
        .expect("second advertiser summary");
    assert!(first_summary.latest_recovery_candidate.is_some());
    assert!(second_summary.latest_recovery_candidate.is_some());

    // The first advertiser serves its summary but withholds the candidate
    // fetch (503), and it is always selected first by the discovery order.
    let transport = RouterFederationTransportRegistry::default();
    transport.insert(
        first_member.federation_api_url.clone(),
        Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .route_layer(axum::middleware::from_fn(
                |req: axum::extract::Request, next: axum::middleware::Next| async move {
                    if req.uri().path().ends_with("/checkpoints/fetch") {
                        return axum::http::Response::builder()
                            .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                            .body(axum::body::Body::empty())
                            .expect("withholding response");
                    }
                    next.run(req).await
                },
            ))
            .with_state(first_state.clone()),
    );
    transport.insert(
        second_member.federation_api_url.clone(),
        Router::new()
            .nest("/api/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );
    let client = FederationRestPeerClient::new(transport, &public_config, second_private)
        .expect("peer client");
    let peer_summaries = [
        (first_member, first_summary),
        (second_member, second_summary),
    ];

    let adopted =
        sync_loop::adopt_peer_recovery_candidate(&recovering_state, &client, &peer_summaries).await;

    assert_eq!(
        adopted, 1,
        "one withholding advertiser must not pin recovery: another advertiser must serve the candidate"
    );
    assert!(
        recovering_state
            .mint
            .localstore()
            .get_federation_checkpoint(checkpoint_id)
            .await
            .expect("recovery candidate lookup")
            .is_some(),
        "the verified candidate must be persisted from an honest advertiser"
    );
}

#[tokio::test]
async fn permanent_apply_contention_transitions_to_bounded_operator_repairable_state() {
    let _budget_guard = ApplyRetryBudgetOverrideGuard::set(2_000);
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

    // Member 1's database remains locked/contended on every finalized-unit
    // application attempt (a permanent retryable error).
    session
        .first_state
        .federation
        .permanent_federation_apply_contention_for_test();

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&session.second_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = session.second_state.clone();
    let submit_task = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    // Member 2 applies the operation normally.
    assert_eq!(
        wait_for_terminal_operation(&session.second_state, operation_id).await,
        FederationOperationStatus::Applied
    );

    // Member 1 must transition to a bounded operator-repairable state with
    // the blocked context exposed instead of retrying forever.
    let mut first_halt_reason = None;
    time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(crate::FederationSyncStatus::Halted { reason }) =
                session.first_state.federation_sync_status().await
            {
                first_halt_reason = Some(reason);
                break;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("member 1 must reach a bounded transition instead of retrying forever");
    let reason = first_halt_reason.expect("halt reason");
    assert!(
        reason.contains("retry budget exhausted"),
        "halt must name the exhausted apply retry budget: {reason}"
    );

    assert!(
        session
            .second_state
            .federation_aleph_bft_live_session_active()
            .await
    );

    let _ = submit_task.abort();
    let _ = submit_task.await;
}

#[tokio::test]
async fn aleph_bft_duplicate_pending_submit_joins_existing_waiter_without_requeuing() {
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
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        response_waiter_limit: 3,
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let first_state = state.clone();
    let first_envelope = envelope.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);

    let second_state = state.clone();
    let second_submit = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(2)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 2);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.response_cache_lookups, 2);
    assert_eq!(stats.response_cache_hits, 0);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    drop(consensus);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let first_response = first_submit
        .await
        .expect("first submit task")
        .expect("first submit response");
    let second_response = second_submit
        .await
        .expect("second submit task")
        .expect("second submit response");
    assert_eq!(first_response.operation_id, operation_id);
    assert_eq!(second_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_accepted_submit_waits_until_state_application_finishes() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    let accepted_response = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(envelope.clone())
            .expect("queue AlephBFT envelope");
        let unit = consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit");
        let mut responses = consensus
            .finalize_aleph_bft_unit(unit)
            .expect("accept AlephBFT unit");
        assert_eq!(responses.len(), 1);
        let response = responses.pop().expect("accepted response");
        assert_eq!(
            response.outcome.entry().status(),
            FederationOperationStatus::Accepted
        );
        response
    };

    let duplicate_state = state.clone();
    let duplicate_submit = tokio::spawn(async move {
        duplicate_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert!(
        !duplicate_submit.is_finished(),
        "an accepted journal entry must not return before its state transition is applied"
    );

    {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        consensus
            .lock()
            .await
            .update_status(operation_id, FederationOperationStatus::Applied)
            .expect("mark operation applied");
    }
    let applied_response = state
        .federation_submit_responses_with_current_status(&[accepted_response])
        .await
        .pop()
        .expect("applied response");
    state
        .notify_federation_operation(applied_response.clone())
        .await;

    let duplicate_response = duplicate_submit
        .await
        .expect("duplicate submit task")
        .expect("duplicate submit response");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert_eq!(
        duplicate_response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_response_cache_eviction_returns_journal_duplicate() {
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
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        response_cache_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_state = state.clone();
    let first_submit = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope.clone())
            .await
    });
    let first_unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(first_unit)
        .await
        .expect("apply first finalized unit");
    let first_submit_response = first_submit
        .await
        .expect("first submit task")
        .expect("first submit response");
    assert_eq!(first_submit_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let second_state = state.clone();
    let second_submit = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(second_envelope)
            .await
    });
    let second_unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(second_unit)
        .await
        .expect("apply second finalized unit");
    let second_submit_response = second_submit
        .await
        .expect("second submit task")
        .expect("second submit response");
    assert_eq!(second_submit_response.operation_id, second_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let duplicate_response = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &first_quote_id))
        .await
        .expect("duplicate submit after cache eviction");
    assert_eq!(duplicate_response.operation_id, first_operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 2);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(first_operation_id)
            .expect("first operation remains durable")
            .operation_id,
        first_operation_id
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_lost_waiters_returns_journal_duplicate() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let submit = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    let accepted_response = submit.await.expect("submit task").expect("submit response");
    assert_eq!(accepted_response.operation_id, operation_id);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    // Simulate a restart after accepted apply where in-memory waiters/cache are gone.
    state.federation.notifications =
        Some(Arc::new(crate::FederationOperationNotifications::default()));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);

    let duplicate_response = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("duplicate submit after lost waiters");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("operation remains durable")
            .operation_id,
        operation_id
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.response_cache_lookups, 0);
}

#[tokio::test]
async fn aleph_bft_duplicate_after_sqlite_restart_returns_journal_duplicate_after_apply_without_response(
) {
    let (public_config, local_private, _) = transport_config();
    let db_file = TempMintDbFile::new("cdk-axum-aleph-bft-applied-no-response-restart");
    let db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("mint db"),
    );
    let seed = Mnemonic::generate(12)
        .expect("mnemonic")
        .to_seed_normalized("");
    let mut state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = state
        .submit_federation_envelope_to_consensus(envelope.clone())
        .await
        .expect_err("unfinalized submit should time out before response delivery");
    assert_pending_finalization_error(&err, operation_id, 10);
    assert_eq!(state.federation_pending_response_waiter_count(), 0);

    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after no-waiter apply")
        .is_some());
    let applied_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after no-waiter apply");
    let applied_digest = materialized_state_digest(&state, &applied_entries)
        .await
        .expect("state digest after no-waiter apply");
    drop(state);
    drop(db);

    let restarted_db = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(db_file.path())
            .await
            .expect("restarted mint db"),
    );
    let mut restarted_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private,
        restarted_db,
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("replay restarted AlephBFT runtime");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    assert_eq!(
        restarted_state.federation_pending_response_waiter_count(),
        0
    );
    assert_eq!(restarted_state.federation_response_cache_entry_count(), 0);
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after restart replay")
        .is_some());

    let duplicate_response = restarted_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("duplicate submit after SQLite restart");
    assert_eq!(duplicate_response.operation_id, operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(
        restarted_state.federation_pending_response_waiter_count(),
        0
    );
    assert_eq!(restarted_state.federation_response_cache_entry_count(), 0);
    let consensus = restarted_state
        .federation
        .consensus
        .as_ref()
        .expect("restarted consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("operation remains durable")
            .operation_id,
        operation_id
    );
    assert!(restarted_state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("pending operations after duplicate restart")
        .is_empty());
    let restarted_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after applied-without-response restart");
    assert_eq!(
        journal_convergence_projection(&restarted_entries),
        journal_convergence_projection(&applied_entries)
    );
    assert_eq!(
        materialized_state_digest(&restarted_state, &restarted_entries)
            .await
            .expect("state digest after applied-without-response restart"),
        applied_digest
    );
}

#[tokio::test]
async fn aleph_bft_duplicate_after_admission_race_returns_journal_duplicate() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .validate_federation_envelope_admission(&envelope)
        .await
        .expect("operation admitted before competing acceptance");

    let competing_state = state.clone();
    let competing_envelope = envelope.clone();
    let competing_submit = tokio::spawn(async move {
        competing_state
            .submit_federation_envelope_to_consensus(competing_envelope)
            .await
    });
    let unit = wait_for_local_aleph_bft_unit(&state).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    let accepted = competing_submit
        .await
        .expect("competing submit task")
        .expect("competing submit response");
    assert_eq!(accepted.operation_id, operation_id);

    let duplicate = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("post-admission race submit should return duplicate");
    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("accepted operation remains durable")
            .operation_id,
        operation_id
    );
    drop(consensus);

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
}

#[tokio::test]
async fn aleph_bft_submit_envelope_rejects_when_pending_mempool_is_full() {
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
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(10),
        pending_mempool_item_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let queued_quote_id = QuoteId::new();
    let queued_envelope = mint_quote_envelope(&state, &queued_quote_id);
    let queued_operation_id = queued_envelope.operation_id().expect("queued operation id");
    {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("consensus handle");
        consensus
            .lock()
            .await
            .submit_aleph_bft_envelope(queued_envelope.clone())
            .expect("prequeued AlephBFT operation");
    }
    let rejected_quote_id = QuoteId::new();

    let err = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &rejected_quote_id))
        .await
        .expect_err("full pending mempool should reject new operation");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::PendingConsensusMempoolFull { pending: 1, max: 1 })
    ));
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&rejected_quote_id)
        .await
        .expect("quote lookup after mempool rejection")
        .is_none());
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    assert!(consensus.journal().entries().is_empty());
    drop(consensus);

    let duplicate_err = state
        .submit_federation_envelope_to_consensus(queued_envelope)
        .await
        .expect_err("duplicate pending operation should wait on existing queue entry");
    assert_pending_finalization_error(&duplicate_err, queued_operation_id, 10);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 1);
    let retained_unit = consensus
        .next_aleph_bft_unit_payload()
        .expect("drain retained AlephBFT unit")
        .expect("retained AlephBFT unit");
    assert_eq!(
        retained_unit
            .items
            .first()
            .expect("retained consensus item")
            .operation_id()
            .expect("retained operation id"),
        Some(queued_operation_id)
    );
    drop(consensus);

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(retained_unit)
        .await
        .expect("apply retained finalized unit");
    assert_eq!(responses.len(), 1);
    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&queued_quote_id)
        .await
        .expect("quote lookup after retained operation finalizes")
        .expect("queued mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{queued_quote_id}"));
    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 1);
    assert_eq!(
        stored_finalized_items[0].operation_id,
        Some(queued_operation_id)
    );
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .first()
            .expect("journal entry")
            .operation_id,
        queued_operation_id
    );
}

#[tokio::test]
async fn aleph_bft_submit_envelope_returns_finalized_duplicate_without_requeuing() {
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
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = queue_local_aleph_bft_unit(&state, envelope.clone()).await;
    state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    let duplicate = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("duplicate response");
    let second_duplicate = state
        .submit_federation_envelope_to_consensus(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("second duplicate response");

    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(second_duplicate.operation_id, operation_id);
    assert!(matches!(
        second_duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable DKG sequences after operation duplicates");
    assert!(
        sequences.is_empty(),
        "operation duplicates must not mutate durable DKG replay state"
    );
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 2);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.queued_len(), 0);
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn startup_replays_accepted_but_unapplied_journal_entry_to_same_state_digest() {
    let (public_config, local_private, _) = transport_config();
    let control_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let mut restarted_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&control_state, &quote_id);
    control_state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("control submit");
    let control_status = local_peer_status(&control_state)
        .await
        .expect("control status");
    let control_digest = materialized_state_digest(&control_state, &[])
        .await
        .expect("control materialized-state digest");
    assert!(control_status.materialized_state_digest.is_none());

    let crashed_history =
        finalized_aleph_bft_history(&public_config, local_private.member_id, [envelope]);
    persist_finalized_aleph_bft_history(&restarted_state, &crashed_history).await;

    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before replay")
        .is_none());
    let missing_status = local_peer_status(&restarted_state)
        .await
        .expect("missing materialized status");
    let missing_digest = materialized_state_digest(&restarted_state, &[])
        .await
        .expect("missing materialized-state digest");
    assert!(missing_status.materialized_state_digest.is_none());
    assert_ne!(
        missing_digest, control_digest,
        "the explicit operator-style digest must detect the unapplied state"
    );

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("runtime state replay");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    let replayed_quote = restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after replay")
        .expect("mint quote replayed");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(replayed_entries.len(), 1);
    assert_eq!(
        replayed_entries[0].status(),
        FederationOperationStatus::Applied
    );

    let replayed_status = local_peer_status(&restarted_state)
        .await
        .expect("replayed status");
    assert_eq!(replayed_status.operation_count, 1);
    assert!(replayed_status.materialized_state_digest.is_none());
    let replayed_digest = materialized_state_digest(&restarted_state, &replayed_entries)
        .await
        .expect("replayed materialized-state digest");
    assert_eq!(
        replayed_digest, control_digest,
        "startup replay must restore the same explicit materialized-state digest"
    );
    let replayed_consensus = restarted_state
        .federation
        .consensus
        .as_ref()
        .expect("replayed consensus")
        .lock()
        .await;
    assert_eq!(replayed_consensus.peer_status().operation_count, 1);
    assert_eq!(
        replayed_consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn router_startup_replays_accepted_journal_before_serving_local_status() {
    let (public_config, local_private, peer_private) = transport_config();
    let control_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let restarted_state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&control_state, &quote_id);
    control_state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("control submit");
    let control_digest = materialized_state_digest(&control_state, &[])
        .await
        .expect("control materialized-state digest");

    let crashed_history =
        finalized_aleph_bft_history(&public_config, local_private.member_id, [envelope]);
    persist_finalized_aleph_bft_history(&restarted_state, &crashed_history).await;
    assert!(restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before router startup")
        .is_none());

    let router = crate::create_mint_router(
        restarted_state.mint.clone(),
        vec![KnownMethod::Bolt11.to_string()],
    )
    .await
    .expect("mint router");
    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_private.member_id,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::POST)
                .uri("/federation/v1/local-status")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&request).expect("request json"),
                ))
                .expect("local status request"),
        )
        .await
        .expect("local status response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("local status body");
    let local_status: FederationLocalStatusResponse =
        serde_json::from_slice(&body).expect("local status json");

    assert_eq!(local_status.peer_status.operation_count, 1);
    assert_eq!(
        local_status.peer_status.materialized_state_digest,
        Some(control_digest)
    );
    assert!(
        local_status
            .materialized_state_rows
            .iter()
            .any(|row| row.table == "mint_quotes"),
        "router startup should materialize the accepted mint quote before serving status"
    );

    let replayed_quote = restarted_state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after router startup")
        .expect("mint quote replayed");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = restarted_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(
        replayed_entries[0].status(),
        FederationOperationStatus::Applied
    );
}
