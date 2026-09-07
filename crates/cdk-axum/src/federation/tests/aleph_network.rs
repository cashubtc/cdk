use super::*;

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_with_duplicate_and_delayed_private_rest_messages() {
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
    let transport = RouterFederationTransportRegistry::default()
        .with_aleph_bft_network_message_duplicate_replays(16)
        .with_aleph_bft_network_message_delays([
            Duration::from_millis(45),
            Duration::ZERO,
            Duration::from_millis(20),
            Duration::ZERO,
            Duration::from_millis(35),
            Duration::ZERO,
        ]);
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
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
        .expect("live AlephBFT operation response under transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
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
async fn aleph_bft_live_session_rejects_malformed_signed_network_message_without_consuming_sequence(
) {
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
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
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
        second_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let malformed_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        second_private.member_id,
        1,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, vec![1, 2, 3]),
        &first_private,
    )
    .expect("signed malformed AlephBFT network-message request");
    let malformed_url = url::Url::parse(&format!("{second_url}/federation/v1/aleph-bft/messages"))
        .expect("malformed AlephBFT network-message URL");
    let malformed_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(malformed_url, None, &malformed_request)
        .await;
    let malformed_error = malformed_result.expect_err("malformed network message must fail");
    match malformed_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("Invalid federation AlephBFT network message"),
                "unexpected malformed-message error: {message}"
            );
        }
        err => panic!("unexpected malformed network-message error: {err:?}"),
    }
    assert_eq!(
        second_state
            .mint
            .localstore()
            .get_federation_dkg_member_sequences()
            .await
            .expect("sequences after malformed network message")
            .get(&first_private.member_id),
        None,
        "a rejected malformed network message must not consume the sender sequence"
    );

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&first_state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("live AlephBFT operation response after malformed message");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
    }
    assert!(
        second_state
            .mint
            .localstore()
            .get_federation_dkg_member_sequences()
            .await
            .expect("durable sequences after valid finality")
            .is_empty(),
        "liveness-only network-message sequences should remain in memory"
    );
}

#[tokio::test]
async fn aleph_bft_live_session_rejects_same_sequence_equivocated_network_message() {
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
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
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
        second_url.clone(),
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
        .expect("live AlephBFT operation response before equivocation");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, operation_id).await;
    wait_for_applied_operation(&second_state, operation_id).await;
    let accepted_request = wait_for_aleph_bft_network_message_request(
        &transport,
        first_private.member_id,
        second_private.member_id,
    )
    .await;
    accepted_request
        .verify(&public_config, accepted_request.receiver)
        .expect("captured network-message request verifies");

    let mut equivocated_message = accepted_request
        .payload
        .message_bytes()
        .expect("captured AlephBFT message bytes");
    equivocated_message.push(0);
    let equivocated_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        second_private.member_id,
        accepted_request.sequence,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, equivocated_message),
        &first_private,
    )
    .expect("signed equivocated AlephBFT network-message request");
    assert_ne!(
        equivocated_request.payload.encoded_message,
        accepted_request.payload.encoded_message
    );
    let equivocated_url =
        url::Url::parse(&format!("{second_url}/federation/v1/aleph-bft/messages"))
            .expect("equivocated AlephBFT network-message URL");
    let equivocated_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(equivocated_url, None, &equivocated_request)
        .await;
    let equivocated_error =
        equivocated_result.expect_err("same-sequence equivocated network message must fail");
    match equivocated_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("stale sequence"),
                "unexpected equivocation error: {message}"
            );
        }
        err => panic!("unexpected equivocated network-message error: {err:?}"),
    }
    assert_eq!(
        second_state
            .mint
            .localstore()
            .get_federation_dkg_member_sequences()
            .await
            .expect("durable sequences after equivocated network message")
            .get(&first_private.member_id),
        None,
        "liveness-only network-message sequences should not be durably persisted"
    );
    for state in [&first_state, &second_state] {
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries after equivocated network message")
                .len(),
            1
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after equivocated network message")
                .finalized_item_count,
            1
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_with_reordered_private_rest_messages() {
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
    let transport =
        RouterFederationTransportRegistry::default().with_aleph_bft_network_message_reorder_once();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(10),
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
        .expect("live AlephBFT operation response under reordered transport");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(transport.aleph_bft_network_message_reorders_completed(), 1);
    for state in [&first_state, &second_state] {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
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
async fn aleph_bft_live_five_member_sessions_finalize_with_reorder_duplicate_and_delay_faults() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
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
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1))
        .with_aleph_bft_network_message_reorder_once()
        .with_aleph_bft_network_message_duplicate_replays(32)
        .with_aleph_bft_network_message_delays([
            Duration::from_millis(45),
            Duration::ZERO,
            Duration::from_millis(25),
            Duration::from_millis(10),
            Duration::ZERO,
            Duration::from_millis(35),
            Duration::ZERO,
            Duration::from_millis(15),
        ]);
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &states[index],
            sync_config,
            pump_config,
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
    assert_eq!(live_session_handles.len(), 5);

    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect("five-member AlephBFT operation response under combined transport faults");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(transport.aleph_bft_network_message_reorders_completed(), 1);
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after combined faults")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_reject_same_sequence_equivocation_and_continue() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
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
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &states[index],
            sync_config,
            pump_config,
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
    assert_eq!(live_session_handles.len(), 5);

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first five-member AlephBFT operation response");

    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, first_operation_id).await;
    }

    let accepted_request = wait_for_aleph_bft_network_message_request(
        &transport,
        private_configs[0].member_id,
        private_configs[1].member_id,
    )
    .await;
    accepted_request
        .verify(&public_config, accepted_request.receiver)
        .expect("captured five-member network-message request verifies");
    assert!(states[1]
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences before equivocated network message")
        .is_empty());

    let mut equivocated_message = accepted_request
        .payload
        .message_bytes()
        .expect("captured AlephBFT message bytes");
    equivocated_message.push(0);
    let equivocated_request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        private_configs[1].member_id,
        accepted_request.sequence,
        FederationRestMessageKind::AlephBftNetworkMessage,
        FederationAlephBftNetworkMessageRequest::new(0, equivocated_message),
        &private_configs[0],
    )
    .expect("signed five-member equivocated AlephBFT network-message request");
    assert_ne!(
        equivocated_request.payload.encoded_message,
        accepted_request.payload.encoded_message
    );
    let equivocated_url = url::Url::parse(&format!(
        "{}/federation/v1/aleph-bft/messages",
        federation_api_urls[1]
    ))
    .expect("equivocated AlephBFT network-message URL");
    let equivocated_result: Result<FederationAlephBftNetworkMessageResponse, HttpError> = transport
        .http_post(equivocated_url, None, &equivocated_request)
        .await;
    let equivocated_error = equivocated_result
        .expect_err("five-member same-sequence equivocated network message must fail");
    match equivocated_error {
        HttpError::Status { status, message } => {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST.as_u16());
            assert!(
                message.contains("stale sequence"),
                "unexpected five-member equivocation error: {message}"
            );
        }
        err => panic!("unexpected five-member equivocated network-message error: {err:?}"),
    }
    assert!(states[1]
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("durable sequences after equivocated network message")
        .is_empty());
    for state in &states {
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal entries after equivocated network message")
                .len(),
            1
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after equivocated network message")
                .finalized_item_count,
            1
        );
    }

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&states[2], &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = states[2]
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second five-member AlephBFT operation response after equivocation");

    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal entries after second operation");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after second operation")
                .finalized_item_count,
            2
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_converge_after_valid_increasing_sequence_fork() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 500);
    let mut session = start_five_member_aleph_bft_live_session(live_options.clone())
        .await
        .expect("five-member live session");
    let bootstrap_activity = mint_quote_envelope(&session.states[0], &QuoteId::new())
        .operation_id()
        .expect("bootstrap activity operation id");
    for state in &session.states {
        state
            .mark_aleph_bft_peer_activity(0, bootstrap_activity)
            .await
            .expect("mark bootstrap AlephBFT activity");
    }
    wait_for_aleph_bft_backup_bytes(&session.states[0], 0).await;
    for handle in &session._handles {
        handle.clear_network_activity();
    }
    time::sleep(Duration::from_millis(100)).await;
    let duplicated_member_backup = session.states[0]
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_bytes(0)
        .await
        .expect("settled duplicated member backup lookup")
        .expect("settled duplicated member backup");

    *session.states[0]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[0]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    drop(session._handles.remove(0));

    let honest_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("honest db"));
    let honest_mnemonic = Mnemonic::generate(12).expect("honest mnemonic");
    let honest_state = federation_route_state_for_private_with_db_and_aleph_bft(
        session.public_config.clone(),
        session.private_configs[0].clone(),
        honest_db,
        &honest_mnemonic.to_seed_normalized(""),
    )
    .await;
    seed_aleph_bft_backup(&honest_state, 0, &duplicated_member_backup).await;
    let honest_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &honest_state,
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("start restored honest member session")
    .expect("restored honest member live handle");
    session.states[0] = honest_state;
    session._handles.insert(0, honest_handle);
    session.transport.insert(
        session.federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[0].clone()),
    );

    let fork_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("fork db"));
    let fork_mnemonic = Mnemonic::generate(12).expect("fork mnemonic");
    let fork_state = federation_route_state_for_private_with_db_and_aleph_bft(
        session.public_config.clone(),
        session.private_configs[0].clone(),
        fork_db,
        &fork_mnemonic.to_seed_normalized(""),
    )
    .await;
    seed_aleph_bft_backup(&fork_state, 0, &duplicated_member_backup).await;
    let fork_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &fork_state,
        session.sync_config,
        live_options,
        session.transport.clone(),
    )
    .await
    .expect("start forked member session")
    .expect("forked member live handle");
    let duplicated_member = session.private_configs[0].member_id;
    session
        .transport
        .set_member_aleph_bft_data_hold(duplicated_member, true);
    session.transport.insert_shadow(
        session.federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(fork_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&session.states[0], &first_quote_id);
    let first_operation_id = first_envelope
        .operation_id()
        .expect("first fork operation id");
    let fork_quote_id = QuoteId::new();
    let fork_envelope = mint_quote_envelope(&fork_state, &fork_quote_id);
    let fork_operation_id = fork_envelope
        .operation_id()
        .expect("second fork operation id");
    assert_ne!(first_operation_id, fork_operation_id);

    let first_submit_state = session.states[0].clone();
    let first_submit = tokio::spawn(async move {
        first_submit_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    let fork_submit_state = fork_state.clone();
    let fork_submit = tokio::spawn(async move {
        fork_submit_state
            .submit_federation_envelope_to_consensus(fork_envelope)
            .await
    });
    let (first_unit_index, fork_unit_index) = tokio::join!(
        wait_for_aleph_bft_unit_with_operation(
            &session.transport,
            duplicated_member,
            first_operation_id,
        ),
        wait_for_aleph_bft_unit_with_operation(
            &session.transport,
            duplicated_member,
            fork_operation_id,
        ),
    );
    let first_unit_index = first_unit_index.expect("honest fork branch must enter transport");
    let fork_unit_index = fork_unit_index.expect("second fork branch must enter transport");

    assert_eq!(
        first_unit_index, fork_unit_index,
        "fork branches must use the same local unit index"
    );
    assert!(!first_submit.is_finished());
    assert!(!fork_submit.is_finished());

    let remote_members = session
        .public_config
        .members
        .values()
        .filter(|member| member.member_id != duplicated_member)
        .cloned()
        .collect::<Vec<_>>();
    let branch_payloads = time::timeout(Duration::from_secs(5), async {
        loop {
            let requests = session.transport.aleph_bft_network_message_requests();
            let selected = remote_members
                .iter()
                .enumerate()
                .map(|(index, member)| {
                    let operation_id = if index % 2 == 0 {
                        first_operation_id
                    } else {
                        fork_operation_id
                    };
                    requests
                        .iter()
                        .find(|request| {
                            request.sender == duplicated_member
                                && request.receiver == member.member_id
                                && aleph_bft_network_request_contains_operation(
                                    request,
                                    operation_id,
                                )
                        })
                        .map(|request| (member.clone(), request.payload.clone()))
                })
                .collect::<Option<Vec<_>>>();
            if let Some(selected) = selected {
                return selected;
            }
            time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both fork variants must be captured for their target peers");

    *fork_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *fork_state.federation.aleph_bft_live_session.write().await = None;
    drop(fork_handle);
    fork_submit.abort();
    session
        .transport
        .remove_shadow(&session.federation_api_urls[0]);
    session
        .transport
        .set_member_aleph_bft_data_hold(duplicated_member, false);
    let replay_client = FederationRestPeerClient::new_with_config(
        session.transport.clone(),
        &session.public_config,
        session.private_configs[0].clone(),
        session.sync_config,
    )
    .expect("fork replay peer client");
    let mut replay_sequences = Vec::new();
    for (member, payload) in branch_payloads {
        let request_count = session.transport.aleph_bft_network_message_requests().len();
        replay_client
            .submit_aleph_bft_network_message(&member, &payload)
            .await
            .expect("deliver authenticated AlephBFT fork branch");
        let replay_sequence = session
            .transport
            .aleph_bft_network_message_requests()
            .into_iter()
            .skip(request_count)
            .find(|request| {
                request.sender == duplicated_member
                    && request.receiver == member.member_id
                    && request.payload == payload
            })
            .expect("freshly signed fork delivery request")
            .sequence;
        replay_sequences.push(replay_sequence);
    }
    assert!(
        replay_sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "fork branches must use distinct increasing private REST sequences"
    );

    let follow_up_quote_id = QuoteId::new();
    let follow_up_envelope = mint_quote_envelope(&session.states[1], &follow_up_quote_id);
    let follow_up_operation_id = follow_up_envelope
        .operation_id()
        .expect("follow-up operation id");
    let follow_up = time::timeout(
        Duration::from_secs(40),
        session.states[1].submit_federation_envelope_to_consensus(follow_up_envelope),
    )
    .await;
    let follow_up = match follow_up {
        Ok(response) => response.expect("honest follow-up response"),
        Err(_) => {
            let mut diagnostics = Vec::with_capacity(session.states.len());
            for state in &session.states {
                let config = state.mint.federation_config().expect("federation config");
                let aleph_bft = state.federation_aleph_bft_status().await;
                let sync = state.federation_sync_status().await;
                let journal_operation_ids = state
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("journal diagnostics")
                    .into_iter()
                    .map(|entry| entry.operation_id)
                    .collect::<Vec<_>>();
                diagnostics.push(format!(
                    "member={} aleph_bft={aleph_bft:?} sync={sync:?} journal_operation_ids={journal_operation_ids:?}",
                    config.member_id()
                ));
            }
            panic!(
                "honest follow-up must finish after fork: {}",
                diagnostics.join("; ")
            );
        }
    };
    assert_eq!(follow_up.operation_id, follow_up_operation_id);
    assert_eq!(
        follow_up.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for state in &session.states {
        wait_for_applied_operation(state, follow_up_operation_id).await;
    }
    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected post-fork journal");
    let expected_operation_ids = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    assert!(
        [first_operation_id, fork_operation_id]
            .iter()
            .filter(|operation_id| expected_operation_ids.contains(operation_id))
            .count()
            <= 1,
        "two variants of one AlephBFT unit must not both enter the journal"
    );
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected post-fork state digest");
    assert!(expected_operation_ids.contains(&follow_up_operation_id));
    for state in &session.states[1..] {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("post-fork journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("post-fork state digest"),
            expected_digest
        );
    }

    first_submit.abort();
    fork_submit.abort();
}

#[tokio::test]
async fn aleph_bft_live_sessions_with_unavailable_member_stall_safely_without_durable_mutation() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs.into_iter().take(4) {
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
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &states[index],
            sync_config,
            pump_config,
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unavailable live member should leave operation pending");

    assert_pending_finalization_error(&err, operation_id, 250);
    for state in &states {
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            0
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_after_unavailable_member_returns() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
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
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &states[index],
            sync_config,
            pump_config,
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

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unavailable member should leave operation pending first");
    assert_pending_finalization_error(&err, operation_id, 250);

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &states[4],
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    });
    let retry_envelope = mint_quote_envelope(&states[0], &quote_id);
    let response = states[0]
        .submit_federation_envelope_to_consensus(retry_envelope)
        .await
        .expect("returned member should allow live finality");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, operation_id).await;
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after finality")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_below_finality_conflicts_and_melt_recover_without_leaking_effects() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = FederationAlephBftLiveSessionOptions::new(pump_config);
    let payment_policy = FederationPaymentObservationPolicy::member_quorum(4);
    let (mut session, make_payment_calls) =
        start_multi_member_aleph_bft_live_session_with_payment_policy(
            5,
            3,
            live_options.clone(),
            payment_policy,
        )
        .await
        .expect("five-member below-finality value session");
    let runtime_config = session.states[0]
        .mint
        .federation_config()
        .expect("below-finality federation config")
        .clone();
    let signature_threshold =
        usize::from(session.public_config.threshold_params.signature_threshold);

    let mint_quote_id = QuoteId::new();
    let mint_quote_envelope = runtime_config.mint_quote_operation(
        mint_quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(4),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: mint_quote_id.to_string(),
            request: format!("invoice-{mint_quote_id}"),
            amount: Some(Amount::from(4)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    );
    let mint_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_quote_envelope)
            .await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            mint_quote_operation_id,
            "below-finality baseline mint quote",
        )
        .await;
    }
    for observer in 1..=4 {
        let observation = mint_payment_observation_envelope_for_member(
            &session.states[0],
            &mint_quote_id,
            observer,
            4,
            "below-finality-mint-paid",
        );
        let operation_id =
            submit_live_operation_allowing_pending_timeout(&session.states[0], observation).await;
        for state in &session.states {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                "below-finality baseline mint payment observation",
            )
            .await;
        }
    }
    let keyset = session.public_config.keysets[0].clone();
    let mint_secrets = (0..4)
        .map(|index| cdk::secret::Secret::new(format!("below-finality proof {index}")))
        .collect::<Vec<_>>();
    let mint_outputs = mint_secrets
        .iter()
        .map(|secret| {
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(secret.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    let mint_envelope = runtime_config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id,
            outputs: mint_outputs.clone(),
            signature: None,
        }))
        .expect("below-finality baseline mint operation");
    let mint_operation_id = mint_envelope
        .operation_id()
        .expect("below-finality baseline mint operation id");
    submit_live_operation_allowing_pending_timeout(&session.states[0], mint_envelope).await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            mint_operation_id,
            "below-finality baseline mint",
        )
        .await;
    }
    let mut proofs = Vec::new();
    for (index, (output, secret)) in mint_outputs.iter().zip(mint_secrets).enumerate() {
        proofs.push(
            proof_from_accepted_output(
                &session.states,
                signature_threshold,
                mint_operation_id,
                u32::try_from(index).expect("below-finality mint output index"),
                output,
                secret,
            )
            .await,
        );
    }

    let melt_quote_id = QuoteId::new();
    let pending_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_envelope = test_melt_quote_operation(
        &runtime_config,
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(
                1_000,
                serde_json::to_string(&pending_description)
                    .expect("below-finality melt description"),
            ),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_quote_envelope)
            .await;
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            melt_quote_operation_id,
            "below-finality baseline melt quote",
        )
        .await;
    }

    let swap_input = proofs.remove(0);
    let swap_input_y = swap_input.y().expect("below-finality swap input y");
    let first_swap_output = swap_output(&session.states[0], "below-finality-swap-first");
    let second_swap_output = swap_output(&session.states[0], "below-finality-swap-second");
    let first_swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input.clone()],
            vec![first_swap_output.clone()],
        ))
        .expect("first below-finality swap envelope");
    let first_swap_operation_id = first_swap_envelope
        .operation_id()
        .expect("first below-finality swap operation id");
    let second_swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![second_swap_output.clone()],
        ))
        .expect("second below-finality swap envelope");
    let second_swap_operation_id = second_swap_envelope
        .operation_id()
        .expect("second below-finality swap operation id");

    let melt_inputs = proofs;
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("below-finality melt input y"))
        .collect::<Vec<_>>();
    let mut melt_change = swap_output(&session.states[0], "below-finality-melt-change");
    melt_change.amount = Amount::ZERO;
    let melt_envelope = runtime_config
        .melt_operation(cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            melt_inputs,
            Some(vec![melt_change.clone()]),
        ))
        .expect("below-finality melt envelope");
    let payment_actor = melt_envelope
        .verify_authorization(&session.public_config)
        .expect("below-finality melt author");
    let payment_actor_index = session
        .public_config
        .members
        .keys()
        .position(|member_id| *member_id == payment_actor)
        .expect("below-finality payment actor index");
    assert!(payment_actor_index < 3);
    let melt_operation_id = melt_envelope
        .operation_id()
        .expect("below-finality melt operation id");

    for index in [4_usize, 3] {
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
        session.transport.insert(
            session.federation_api_urls[index].clone(),
            Router::new()
                .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
                .with_state(session.states[0].clone()),
        );
    }
    let fifth_handle = session._handles.remove(4);
    let fourth_handle = session._handles.remove(3);
    drop(fifth_handle);
    drop(fourth_handle);

    for index in 0..3 {
        session.states[index].federation.peer_sync_config = Some(FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_millis(250),
            ..session.sync_config
        });
    }
    for (index, envelope, operation_id) in [
        (0_usize, first_swap_envelope, first_swap_operation_id),
        (1_usize, second_swap_envelope, second_swap_operation_id),
        (2_usize, melt_envelope, melt_operation_id),
    ] {
        let err = session.states[index]
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect_err("below-finality operation must remain pending");
        assert_pending_finalization_error(&err, operation_id, 250);
        let pending = session.states[index]
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("below-finality pending operation lookup");
        assert!(pending
            .iter()
            .any(|pending| pending.operation_id == operation_id));
    }

    let pending_operation_ids = [
        first_swap_operation_id,
        second_swap_operation_id,
        melt_operation_id,
    ];
    let pending_blinded_secrets = [
        first_swap_output.blinded_secret,
        second_swap_output.blinded_secret,
        melt_change.blinded_secret,
    ];
    for state in &session.states {
        for operation_id in pending_operation_ids {
            assert!(state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(operation_id)
                .await
                .expect("below-finality journal lookup")
                .is_none());
            assert!(state
                .mint
                .localstore()
                .get_completed_operation(&federation_completed_operation_uuid(operation_id))
                .await
                .expect("below-finality completed operation lookup")
                .is_none());
            assert!(state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(operation_id, 0)
                .await
                .expect("below-finality signature share lookup")
                .is_empty());
        }
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[swap_input_y])
                .await
                .expect("below-finality swap proof state"),
            vec![None]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("below-finality melt proof states"),
            vec![None, None, None]
        );
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures(&pending_blinded_secrets)
            .await
            .expect("below-finality blind signature lookup")
            .iter()
            .all(Option::is_none));
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&melt_quote_id)
                .await
                .expect("below-finality melt quote lookup")
                .expect("below-finality melt quote")
                .state,
            MeltQuoteState::Unpaid
        );
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        0,
        "below-finality melt must not attempt payment"
    );

    replay_aleph_bft_runtime_after_test_process_restart(&mut session.states[3]).await;
    let fourth_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &session.states[3],
        session.sync_config,
        live_options.clone(),
        session.transport.clone(),
    )
    .await
    .expect("restart fourth below-finality member")
    .expect("fourth below-finality live session handle");
    session._handles.push(fourth_handle);
    session.transport.insert(
        session.federation_api_urls[3].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[3].clone()),
    );
    for state in &mut session.states[..4] {
        state.federation.peer_sync_config = Some(session.sync_config);
    }

    let first_status =
        wait_for_terminal_operation(&session.states[0], first_swap_operation_id).await;
    let second_status =
        wait_for_terminal_operation(&session.states[0], second_swap_operation_id).await;
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == FederationOperationStatus::Applied)
            .count(),
        1,
        "exactly one conflicting swap must apply"
    );
    assert_eq!(
        [first_status, second_status]
            .into_iter()
            .filter(|status| *status == FederationOperationStatus::Rejected)
            .count(),
        1,
        "exactly one conflicting swap must be rejected"
    );
    assert_eq!(
        wait_for_terminal_operation(&session.states[0], melt_operation_id).await,
        FederationOperationStatus::Applied
    );
    let (
        winning_swap_operation_id,
        winning_swap_output,
        losing_swap_operation_id,
        losing_swap_output,
    ) = match first_status {
        FederationOperationStatus::Applied => (
            first_swap_operation_id,
            first_swap_output.clone(),
            second_swap_operation_id,
            second_swap_output.clone(),
        ),
        FederationOperationStatus::Rejected => (
            second_swap_operation_id,
            second_swap_output.clone(),
            first_swap_operation_id,
            first_swap_output.clone(),
        ),
        status => panic!("unexpected first swap status: {status}"),
    };

    for state in &session.states[..4] {
        assert_eq!(
            wait_for_terminal_operation(state, first_swap_operation_id).await,
            first_status
        );
        assert_eq!(
            wait_for_terminal_operation(state, second_swap_operation_id).await,
            second_status
        );
        assert_eq!(
            wait_for_terminal_operation(state, melt_operation_id).await,
            FederationOperationStatus::Applied
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(winning_swap_operation_id, 0)
                .await
                .expect("winning swap signature share lookup")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(losing_swap_operation_id, 0)
            .await
            .expect("losing swap signature share lookup")
            .is_empty());
    }

    let mut observation_operation_ids = Vec::new();
    let actor_observation = submit_melt_payment_observation_from_backend(
        &session.states[payment_actor_index],
        &melt_quote_id,
    )
    .await;
    observation_operation_ids.push(actor_observation.operation_id);
    for state in &session.states[..4] {
        wait_for_applied_operation_with_context(
            state,
            actor_observation.operation_id,
            "below-finality author melt payment observation",
        )
        .await;
    }
    for observer_index in (0..4).filter(|observer_index| *observer_index != payment_actor_index) {
        let observation = matching_melt_payment_observation_envelope(
            &session.states[observer_index],
            &actor_observation.observation,
        );
        let operation_id = submit_live_operation_allowing_pending_timeout(
            &session.states[observer_index],
            observation,
        )
        .await;
        observation_operation_ids.push(operation_id);
        for state in &session.states[..4] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                "below-finality melt payment observation",
            )
            .await;
        }
    }
    assert_eq!(
        make_payment_calls[payment_actor_index].load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "recovered melt must retain exactly one payment actor"
    );

    session.states[4]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[4]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("final below-finality lagger must fail closed");
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
        .expect("caught-up below-finality lagger is ready");

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("below-finality expected journal");
    let expected_operation_ids = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let expected_statuses = expected_entries
        .iter()
        .map(|entry| entry.status())
        .collect::<Vec<_>>();
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("below-finality expected state digest");
    let expected_cursor = session.states[0]
        .federation_aleph_bft_status()
        .await
        .expect("below-finality expected cursor")
        .next_item_index;

    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("below-finality converged journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            expected_statuses
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("below-finality converged state digest"),
            expected_digest
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("below-finality converged cursor")
                .next_item_index,
            expected_cursor
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&[swap_input_y])
                .await
                .expect("below-finality final swap proof state"),
            vec![Some(cdk::nuts::State::Spent)]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("below-finality final melt proof states"),
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[
                winning_swap_output.blinded_secret,
                losing_swap_output.blinded_secret,
                melt_change.blinded_secret,
            ])
            .await
            .expect("below-finality final blind signatures");
        assert!(signatures[0].is_some());
        assert!(signatures[1].is_none());
        assert!(signatures[2].is_some());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(winning_swap_operation_id, 0)
                .await
                .expect("below-finality final winning share")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(losing_swap_operation_id, 0)
            .await
            .expect("below-finality final losing share")
            .is_empty());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(melt_operation_id, 0)
                .await
                .expect("below-finality final melt share")
                .len(),
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(
                winning_swap_operation_id,
            ))
            .await
            .expect("below-finality winning completion")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(
                losing_swap_operation_id,
            ))
            .await
            .expect("below-finality losing completion")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(melt_operation_id))
            .await
            .expect("below-finality melt completion")
            .is_some());
        assert_eq!(
            state
                .mint
                .localstore()
                .get_melt_quote(&melt_quote_id)
                .await
                .expect("below-finality final melt quote lookup")
                .expect("below-finality final melt quote")
                .state,
            MeltQuoteState::Paid
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("below-finality final pending operation lookup")
            .is_empty());
    }
    for operation_id in observation_operation_ids {
        assert_eq!(
            wait_for_terminal_operation(&session.states[4], operation_id).await,
            FederationOperationStatus::Applied
        );
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "catch-up must not repeat the recovered melt payment"
    );
    assert_eq!(make_payment_calls[4].load(Ordering::SeqCst), 0);
}
