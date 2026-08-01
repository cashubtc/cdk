use cdk_http_client::Transport;

fn iroh_federation_url(endpoint: cdk_iroh::EndpointId) -> MintUrl {
    MintUrl::from_str(&format!("iroh://{endpoint}")).expect("Iroh federation URL")
}

fn federation_test_router(state: MintState) -> Router {
    Router::new()
        .nest("/federation/v1", create_federation_router())
        .with_state(state)
}

fn spawn_http_federation_server(
    listener: tokio::net::TcpListener,
    state: MintState,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, federation_test_router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("HTTP federation server");
    });
    (shutdown, task)
}

#[tokio::test]
async fn one_hybrid_transport_dials_mixed_http_and_iroh_roster() {
    let first_node = cdk_iroh::IrohNode::ephemeral(cdk_iroh::IrohConfig::static_only())
        .await
        .expect("first Iroh node");
    let third_node = cdk_iroh::IrohNode::ephemeral(
        cdk_iroh::IrohConfig::static_only().with_ticket(first_node.endpoint_ticket()),
    )
    .await
    .expect("third Iroh node");
    first_node.add_ticket(third_node.endpoint_ticket());
    let http_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("HTTP federation listener");
    let http_address = http_listener.local_addr().expect("HTTP listener address");
    let (public, private) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(3, 2).expect("three-member thresholds"),
        vec![
            iroh_federation_url(first_node.endpoint_id()),
            MintUrl::from_str(&format!("http://{http_address}"))
                .expect("HTTP federation URL"),
            iroh_federation_url(third_node.endpoint_id()),
        ],
    );
    let mut first_state =
        federation_route_state_for_private(public.clone(), private[0].clone()).await;
    first_state.federation.transport =
        federation_transport_with_iroh_node(first_node.clone());
    let second_state =
        federation_route_state_for_private(public.clone(), private[1].clone()).await;
    let mut third_state =
        federation_route_state_for_private(public.clone(), private[2].clone()).await;
    third_state.federation.transport =
        federation_transport_with_iroh_node(third_node.clone());

    let first_server = cdk_iroh::IrohServer::start(
        first_node.clone(),
        federation_test_router(first_state),
    );
    let third_server = cdk_iroh::IrohServer::start(
        third_node.clone(),
        federation_test_router(third_state),
    );
    let (http_shutdown, http_shutdown_rx) = tokio::sync::oneshot::channel();
    let http_server = tokio::spawn(async move {
        axum::serve(http_listener, federation_test_router(second_state))
            .with_graceful_shutdown(async {
                let _ = http_shutdown_rx.await;
            })
            .await
            .expect("HTTP federation server");
    });

    let client = FederationRestPeerClient::new(
        federation_transport_with_iroh_node(first_node.clone()),
        &public,
        private[0].clone(),
    )
    .expect("mixed peer client");
    let (http_status, iroh_status) = tokio::join!(
        client.peer_status(public.member(member_id(2)).expect("HTTP member")),
        client.peer_status(public.member(member_id(3)).expect("Iroh member")),
    );
    assert_eq!(http_status.expect("HTTP peer status").member_id, member_id(2));
    assert_eq!(iroh_status.expect("Iroh peer status").member_id, member_id(3));

    http_shutdown.send(()).expect("HTTP shutdown signal");
    http_server.await.expect("HTTP server task");
    third_server.shutdown().await.expect("third Iroh shutdown");
    first_server.shutdown().await.expect("first Iroh shutdown");
}

#[tokio::test]
async fn all_iroh_private_routes_preserve_dkg_signatures_and_replay_fences() {
    let first_node = cdk_iroh::IrohNode::ephemeral(cdk_iroh::IrohConfig::static_only())
        .await
        .expect("first Iroh node");
    let second_node = cdk_iroh::IrohNode::ephemeral(
        cdk_iroh::IrohConfig::static_only().with_ticket(first_node.endpoint_ticket()),
    )
    .await
    .expect("second Iroh node");
    let third_node = cdk_iroh::IrohNode::ephemeral(
        cdk_iroh::IrohConfig::static_only().with_ticket(first_node.endpoint_ticket()),
    )
    .await
    .expect("third Iroh node");
    first_node.add_ticket(second_node.endpoint_ticket());
    first_node.add_ticket(third_node.endpoint_ticket());
    let (public, private) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(3, 2).expect("three-member thresholds"),
        vec![
            iroh_federation_url(first_node.endpoint_id()),
            iroh_federation_url(second_node.endpoint_id()),
            iroh_federation_url(third_node.endpoint_id()),
        ],
    );
    let mut first_state =
        federation_route_state_for_private(public.clone(), private[0].clone()).await;
    first_state.federation.transport =
        federation_transport_with_iroh_node(first_node.clone());
    let mut second_state =
        federation_route_state_for_private(public.clone(), private[1].clone()).await;
    second_state.federation.transport =
        federation_transport_with_iroh_node(second_node.clone());

    let first_server = cdk_iroh::IrohServer::start(
        first_node.clone(),
        federation_test_router(first_state),
    );
    let second_server = cdk_iroh::IrohServer::start(
        second_node.clone(),
        federation_test_router(second_state.clone()),
    );
    let target = public.member(member_id(2)).expect("second member");
    let transport = federation_transport_with_iroh_node(first_node.clone());
    let client = FederationRestPeerClient::new(
        transport.clone(),
        &public,
        private[0].clone(),
    )
    .expect("all-Iroh peer client");

    assert_eq!(
        client
            .peer_status(public.member(member_id(1)).expect("local member"))
            .await
            .expect("Iroh self status")
            .member_id,
        member_id(1)
    );
    assert_eq!(
        client.peer_status(target).await.expect("Iroh peer status").member_id,
        member_id(2)
    );
    let commitment = dkg_commitment_request(&public, member_id(1), 11);
    let reveal = dkg_reveal_request(&public, member_id(1), 11);
    let secret_share = dkg_secret_share_request(&public, member_id(1), member_id(2), 11);
    client
        .submit_dkg_commitment(target, &commitment)
        .await
        .expect("Iroh DKG commitment");
    client
        .submit_dkg_reveal(target, &reveal)
        .await
        .expect("Iroh DKG reveal");
    client
        .submit_dkg_secret_share(target, &secret_share)
        .await
        .expect("Iroh DKG secret share");

    let stored = second_state
        .mint
        .localstore()
        .get_federation_dkg_secret_share_contributions()
        .await
        .expect("stored Iroh DKG shares");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0], secret_share);

    let replay_request = signed_status_request(
        &public,
        &private[0],
        member_id(2),
        u64::MAX - 1,
    );
    let status_url = target
        .federation_api_url
        .join_paths(&["federation", "v1", "status"])
        .expect("status URL");
    let _: FederationPeerStatus = transport
        .http_post(status_url.clone(), None, &replay_request)
        .await
        .expect("first exact signed request");
    let replay: Result<FederationPeerStatus, _> =
        transport.http_post(status_url, None, &replay_request).await;
    assert!(replay.is_err(), "Iroh must not bypass replay rejection");

    second_server.shutdown().await.expect("second Iroh shutdown");
    first_server.shutdown().await.expect("first Iroh shutdown");
    third_node.close().await;
}

#[tokio::test]
async fn mixed_http_iroh_roster_operates_restarts_and_catches_up() {
    let nodes = vec![
        cdk_iroh::IrohNode::ephemeral(cdk_iroh::IrohConfig::static_only())
            .await
            .expect("first client node"),
        cdk_iroh::IrohNode::ephemeral(cdk_iroh::IrohConfig::static_only())
            .await
            .expect("second server node"),
        cdk_iroh::IrohNode::ephemeral(cdk_iroh::IrohConfig::static_only())
            .await
            .expect("third client node"),
    ];
    for source in &nodes {
        for target in &nodes {
            if source.endpoint_id() != target.endpoint_id() {
                source.add_ticket(target.endpoint_ticket());
            }
        }
    }
    let first_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("first HTTP listener");
    let first_address = first_listener.local_addr().expect("first HTTP address");
    let third_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("third HTTP listener");
    let third_address = third_listener.local_addr().expect("third HTTP address");
    let federation_api_urls = vec![
        MintUrl::from_str(&format!("http://{first_address}"))
            .expect("first HTTP federation URL"),
        iroh_federation_url(nodes[1].endpoint_id()),
        MintUrl::from_str(&format!("http://{third_address}"))
            .expect("third HTTP federation URL"),
    ];
    let (public, private) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(3, 2).expect("three-member thresholds"),
        federation_api_urls,
    );
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db(
        public.clone(),
        private[0].clone(),
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let mut second_state =
        federation_route_state_for_private(public.clone(), private[1].clone()).await;
    let mut third_state =
        federation_route_state_for_private(public.clone(), private[2].clone()).await;
    first_state.federation.transport = federation_transport_with_iroh_node(nodes[0].clone());
    second_state.federation.transport = federation_transport_with_iroh_node(nodes[1].clone());
    third_state.federation.transport = federation_transport_with_iroh_node(nodes[2].clone());

    let (first_shutdown, first_task) =
        spawn_http_federation_server(first_listener, first_state.clone());
    let second_server =
        cdk_iroh::IrohServer::start(nodes[1].clone(), federation_test_router(second_state.clone()));
    let (third_shutdown, third_task) =
        spawn_http_federation_server(third_listener, third_state.clone());
    let clients = private
        .iter()
        .cloned()
        .zip(nodes.iter().cloned())
        .map(|(private, node)| {
            FederationRestPeerClient::new(
                federation_transport_with_iroh_node(node),
                &public,
                private,
            )
            .expect("mixed federation peer client")
        })
        .collect::<Vec<_>>();
    for client in &clients {
        for member in public.members.values() {
            assert_eq!(
                client
                    .peer_status(member)
                    .await
                    .expect("directed mixed peer status")
                    .member_id,
                member.member_id
            );
        }
    }

    let quote_id = QuoteId::new();
    second_state
        .submit_federation_envelope(mint_quote_envelope(&second_state, &quote_id))
        .await
        .expect("source operation before target restart");
    let (_, quorum) = recovery_checkpoint_quorum_for_state(
        &second_state,
        &public,
        None,
        &[&private[0], &private[1], &private[2]],
    )
    .await;
    let mut transaction = second_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin source checkpoint promotion");
    transaction
        .promote_verified_federation_checkpoint(
            &quorum.verify(&public).expect("verify source checkpoint quorum"),
        )
        .await
        .expect("promote source checkpoint");
    transaction.commit().await.expect("commit source checkpoint");
    let mut transaction = second_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin source backup transaction");
    transaction
        .replace_federation_aleph_bft_backup_chunks(
            quorum.checkpoint.aleph_bft_session,
            &[vec![1; 16]],
        )
        .await
        .expect("persist source backup");
    transaction
        .set_federation_aleph_bft_active_session(quorum.checkpoint.aleph_bft_session)
        .await
        .expect("persist source active session");
    transaction.commit().await.expect("commit source backup");

    first_shutdown.send(()).expect("stop first HTTP member");
    first_task.await.expect("first HTTP server task");
    drop(first_state);
    let mut restarted_first = federation_route_state_for_private_with_db(
        public.clone(),
        private[0].clone(),
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first.mint)
            .await
            .expect("replay first runtime after restart");
    restarted_first.federation.consensus = replayed_consensus;
    restarted_first.federation.sequences = replayed_sequences;
    restarted_first.federation.transport =
        federation_transport_with_iroh_node(nodes[0].clone());
    let restarted_listener = tokio::net::TcpListener::bind(first_address)
        .await
        .expect("restarted first HTTP listener");
    let (restarted_shutdown, restarted_task) =
        spawn_http_federation_server(restarted_listener, restarted_first.clone());
    let restarted_client = FederationRestPeerClient::new(
        federation_transport_with_iroh_node(nodes[0].clone()),
        &public,
        private[0].clone(),
    )
    .expect("restarted mixed peer client");
    let source_member = public.member(member_id(2)).expect("Iroh source member");
    assert_eq!(
        run_federation_peer_sync_round(
            &restarted_first,
            &restarted_client,
            std::slice::from_ref(source_member),
        )
        .await,
        1
    );
    let source_status = restarted_client
        .sync_once_from_member(&restarted_first, source_member)
        .await
        .expect("catch up restarted HTTP member from Iroh member");
    assert_eq!(source_status.operation_count, 1);
    assert!(restarted_first
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("caught-up quote lookup")
        .is_some());

    restarted_shutdown
        .send(())
        .expect("stop restarted first HTTP member");
    restarted_task
        .await
        .expect("restarted first HTTP server task");
    third_shutdown.send(()).expect("stop third HTTP member");
    third_task.await.expect("third HTTP server task");
    second_server.shutdown().await.expect("second Iroh shutdown");
    nodes[0].close().await;
    nodes[2].close().await;
}
