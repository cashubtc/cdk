use super::*;

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
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db.clone(),
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
    let live_options = fast_aleph_bft_live_session_options(
        CdkAlephBftNetworkPumpConfig {
            session_id: 0,
            max_messages_per_tick: 128,
            max_peer_deliveries_per_message: 128,
        },
        500,
    );
    let _first_handle = start_federation_aleph_bft_live_session_with_options_and_transport(
        &first_state,
        sync_config,
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
        &second_state,
        sync_config,
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
        &first_state,
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
    submit_task.abort();
    let _ = submit_task.await;
}
