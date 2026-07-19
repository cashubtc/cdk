#[tokio::test]
async fn aleph_bft_live_restart_matrix_one_member_continues_finality() {
    run_five_member_restart_case(&[0]).await;
}

#[tokio::test]
async fn aleph_bft_live_restart_matrix_consensus_threshold_continues_finality() {
    run_five_member_restart_case(&[0, 1, 2, 3]).await;
}

#[tokio::test]
async fn aleph_bft_live_restart_matrix_all_members_continues_finality() {
    run_five_member_restart_case(&[0, 1, 2, 3, 4]).await;
}

async fn run_five_member_partition_and_heal(partition_size: usize, finalizes_while_split: bool) {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options(pump_config, 500);
    let mut session = start_five_member_aleph_bft_live_session(live_options)
        .await
        .expect("five-member partition session");

    let baseline_quote_id = QuoteId::new();
    let baseline = mint_quote_envelope(&session.states[0], &baseline_quote_id);
    let baseline_operation_id = baseline.operation_id().expect("baseline operation id");
    session.states[0]
        .submit_federation_envelope_to_consensus(baseline)
        .await
        .expect("baseline operation before partition");
    for state in &session.states {
        wait_for_applied_operation(state, baseline_operation_id).await;
    }

    let first = session.private_configs[..partition_size]
        .iter()
        .map(|config| config.member_id)
        .collect::<Vec<_>>();
    let second = session.private_configs[partition_size..]
        .iter()
        .map(|config| config.member_id)
        .collect::<Vec<_>>();
    session
        .transport
        .set_network_partition(&first, &second, true);
    session.states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(500),
        ..session.sync_config
    });

    let partitioned_quote_id = QuoteId::new();
    let partitioned = mint_quote_envelope(&session.states[0], &partitioned_quote_id);
    let partitioned_operation_id = partitioned
        .operation_id()
        .expect("partitioned operation id");
    let result = session.states[0]
        .submit_federation_envelope_to_consensus(partitioned.clone())
        .await;
    if finalizes_while_split {
        match result {
            Ok(response) => assert_eq!(response.operation_id, partitioned_operation_id),
            Err(err) => assert_pending_finalization_error(&err, partitioned_operation_id, 500),
        }
        for state in &session.states[..partition_size] {
            wait_for_applied_operation(state, partitioned_operation_id).await;
        }
        for state in &session.states[partition_size..] {
            assert!(state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(partitioned_operation_id)
                .await
                .expect("isolated journal lookup")
                .is_none());
        }
    } else {
        let error = result.expect_err("below-threshold side cannot finalize");
        assert_pending_finalization_error(&error, partitioned_operation_id, 500);
        for state in &session.states {
            assert!(state
                .mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(partitioned_operation_id)
                .await
                .expect("split journal lookup")
                .is_none());
        }
    }

    session
        .transport
        .set_network_partition(&first, &second, false);
    session.states[0].federation.peer_sync_config = Some(session.sync_config);
    if !finalizes_while_split {
        let response = session.states[0]
            .submit_federation_envelope_to_consensus(partitioned)
            .await
            .expect("healed partition finalizes pending operation");
        assert_eq!(response.operation_id, partitioned_operation_id);
    }
    for state in &session.states {
        wait_for_applied_operation_with_context(
            state,
            partitioned_operation_id,
            "healed partition convergence",
        )
        .await;
    }
    assert_multi_member_journal_and_state_converged(
        &session,
        &[baseline_operation_id, partitioned_operation_id],
    )
    .await;
}

#[tokio::test]
async fn aleph_bft_live_below_consensus_threshold_partition_heals() {
    run_five_member_partition_and_heal(3, false).await;
}

#[tokio::test]
async fn aleph_bft_live_above_consensus_threshold_partition_heals() {
    run_five_member_partition_and_heal(4, true).await;
}

async fn submit_healthy_federation_swap_batch(
    session: &MultiMemberAlephBftLiveSession,
    first_index: usize,
    count: usize,
    parallelism: usize,
) -> Vec<FederationOperationId> {
    let config = session.states[0]
        .mint
        .federation_config()
        .expect("stress federation config")
        .clone();
    let operations = (first_index..first_index + count)
        .map(|index| {
            let envelope = config
                .swap_operation(cdk::nuts::SwapRequest::new(
                    vec![proof_for_amount_with_aggregate_secret(
                        &session.states[0],
                        1,
                        &format!("healthy-stress-input-{index}"),
                        &BlsSecretKey::from_reduced_bytes(&[31; 32]),
                    )],
                    vec![swap_output(
                        &session.states[0],
                        &format!("healthy-stress-output-{index}"),
                    )],
                ))
                .expect("stress swap envelope");
            let operation_id = envelope.operation_id().expect("stress swap operation id");
            (operation_id, envelope)
        })
        .collect::<Vec<_>>();

    for chunk in operations.chunks(parallelism) {
        let mut submissions = tokio::task::JoinSet::new();
        for (offset, (operation_id, envelope)) in chunk.iter().cloned().enumerate() {
            let state = session.states[(first_index + offset) % session.states.len()].clone();
            submissions.spawn(async move {
                let response = state
                    .submit_federation_envelope_to_consensus(envelope)
                    .await
                    .expect("healthy stress swap submission");
                assert_eq!(response.operation_id, operation_id);
                assert_eq!(
                    response.outcome.entry().status(),
                    FederationOperationStatus::Applied
                );
            });
        }
        while let Some(result) = submissions.join_next().await {
            result.expect("healthy stress swap task");
        }
    }

    operations
        .into_iter()
        .map(|(operation_id, _)| operation_id)
        .collect()
}

fn spawn_healthy_federation_peer_sync_tasks(
    session: &MultiMemberAlephBftLiveSession,
) -> Vec<tokio::task::JoinHandle<()>> {
    session
        .states
        .iter()
        .cloned()
        .zip(session.private_configs.iter().cloned())
        .enumerate()
        .map(|(member_rank, (state, private_config))| {
            let public_config = session.public_config.clone();
            let local_member = private_config.member_id;
            let peers = public_config
                .members
                .values()
                .filter(|member| member.member_id != local_member)
                .cloned()
                .collect::<Vec<_>>();
            let sync_config = session.sync_config;
            let transport = session.transport.clone();
            tokio::spawn(async move {
                let client = FederationRestPeerClient::new_with_config(
                    transport,
                    &public_config,
                    private_config,
                    sync_config,
                )
                .expect("healthy stress peer client");
                let initial_delay = federation_peer_sync_initial_delay(
                    sync_config.peer_sync_interval,
                    member_rank,
                    public_config.members.len(),
                );
                if !initial_delay.is_zero() {
                    time::sleep(initial_delay).await;
                }
                let mut interval = federation_peer_sync_interval(sync_config);
                let mut reconnect_backoff = FederationPeerReconnectBackoff::default();
                loop {
                    interval.tick().await;
                    run_federation_peer_sync_round_with_backoff(
                        &state,
                        &client,
                        &peers,
                        &mut reconnect_backoff,
                    )
                    .await;
                }
            })
        })
        .collect()
}

#[tokio::test]
#[ignore = "process-level federation stress regression"]
async fn aleph_bft_live_healthy_swaps_do_not_trigger_catch_up() {
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let session = start_five_member_aleph_bft_live_session(
        FederationAlephBftLiveSessionOptions::new(pump_config),
    )
    .await
    .expect("five-member healthy stress session");
    for state in &session.states {
        state.restore_federation_startup_readiness().await;
    }
    let sync_tasks = spawn_healthy_federation_peer_sync_tasks(&session);

    let operation_ids = submit_healthy_federation_swap_batch(&session, 0, 5, 1).await;

    for operation_id in &operation_ids {
        for state in &session.states {
            wait_for_applied_operation_with_context(
                state,
                *operation_id,
                "healthy stress convergence",
            )
            .await;
        }
    }
    for task in &sync_tasks {
        task.abort();
    }
    for task in sync_tasks {
        let err = task.await.expect_err("healthy stress peer sync task abort");
        assert!(err.is_cancelled());
    }

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("healthy stress journal");
    assert_eq!(expected_entries.len(), operation_ids.len());
    assert!(expected_entries
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    let expected_order = expected_entries
        .iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("healthy stress materialized digest");
    let mut heights = Vec::new();

    for (member_index, state) in session.states.iter().enumerate() {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("member healthy stress journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_order
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("member healthy stress materialized digest"),
            expected_digest
        );
        let sync_status = state.federation_sync_status().await;
        let transition_stats = state
            .federation_sync_transition_stats()
            .await
            .expect("member sync transition stats");
        assert_eq!(
            transition_stats,
            crate::FederationSyncTransitionStats::default(),
            "healthy load must not enter lagging or catch-up states: member={}, status={sync_status:?}",
            member_index + 1,
        );
        heights.push(
            state
                .federation
                .consensus
                .as_ref()
                .expect("member consensus")
                .lock()
                .await
                .peer_status()
                .operation_count,
        );
    }

    let minimum_height = heights
        .iter()
        .min()
        .copied()
        .expect("minimum member height");
    let maximum_height = heights
        .iter()
        .max()
        .copied()
        .expect("maximum member height");
    assert_eq!(
        maximum_height - minimum_height,
        0,
        "healthy stress members must converge without height skew"
    );
}

async fn submit_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let submit_state = state.clone();
    let response = tokio::spawn(async move {
        submit_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    })
    .await
    .expect("submit task")
    .expect("live AlephBFT operation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    operation_id
}

async fn submit_duplicate_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_state = state.clone();
    let first_envelope = envelope.clone();
    let first = tokio::spawn(async move {
        first_state
            .submit_federation_envelope_to_consensus(first_envelope)
            .await
    });
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        second_state
            .submit_federation_envelope_to_consensus(envelope)
            .await
    });

    let (first_response, second_response) = tokio::join!(first, second);
    let first_response = first_response
        .expect("first submit task")
        .expect("first live AlephBFT operation response");
    let second_response = second_response
        .expect("second submit task")
        .expect("second live AlephBFT operation response");

    for response in [first_response, second_response] {
        assert_eq!(response.operation_id, operation_id);
        assert_eq!(
            response.outcome.entry().status(),
            FederationOperationStatus::Applied
        );
    }

    operation_id
}

async fn wait_for_latency_stage(
    recorder: &CdkAlephBftLatencyRecorder,
    operation_id: FederationOperationId,
    stage: CdkAlephBftLatencyStage,
) {
    time::timeout(
        Duration::from_secs(5),
        recorder.wait_for_stage(operation_id, stage),
    )
    .await
    .expect("AlephBFT latency stage was not recorded before timeout")
}

fn assert_latency_stage_order(
    events: &[CdkAlephBftLatencyEvent],
    expected: &[CdkAlephBftLatencyStage],
) {
    let mut cursor = 0usize;
    for event in events {
        if event.stage == expected[cursor] {
            cursor += 1;
            if cursor == expected.len() {
                return;
            }
        }
    }

    panic!(
        "AlephBFT latency stages were not observed in order; expected {:?}, got {}",
        expected,
        format_latency_events(events)
    );
}

fn latency_stage_delta(
    events: &[CdkAlephBftLatencyEvent],
    start: CdkAlephBftLatencyStage,
    end: CdkAlephBftLatencyStage,
) -> Option<Duration> {
    let start_elapsed = events
        .iter()
        .find(|event| event.stage == start)
        .map(|event| event.elapsed)?;
    let end_elapsed = events
        .iter()
        .find(|event| event.stage == end)
        .map(|event| event.elapsed)?;

    end_elapsed.checked_sub(start_elapsed)
}

fn latency_stage_count(
    events: &[CdkAlephBftLatencyEvent],
    stage: CdkAlephBftLatencyStage,
) -> usize {
    events.iter().filter(|event| event.stage == stage).count()
}

fn format_latency_events(events: &[CdkAlephBftLatencyEvent]) -> String {
    events
        .iter()
        .map(|event| format!("{:?}@{}ms", event.stage, event.elapsed.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn fast_aleph_bft_live_session_options(
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    delay_config.tick_interval = Duration::from_millis(5);
    delay_config.unit_rebroadcast_interval_min = Duration::from_millis(20);
    delay_config.unit_rebroadcast_interval_max = Duration::from_millis(40);
    delay_config.unit_creation_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.coord_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.parent_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.newest_request_delay = Arc::new(|_| Duration::from_millis(10));

    FederationAlephBftLiveSessionOptions::new(pump_config)
        .with_max_round(max_round)
        .with_delay_config(delay_config)
}

fn fast_aleph_bft_live_session_options_with_idle_tail(
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    delay_config.tick_interval = Duration::from_millis(5);
    delay_config.unit_rebroadcast_interval_min = Duration::from_millis(20);
    delay_config.unit_rebroadcast_interval_max = Duration::from_millis(40);
    delay_config.unit_creation_delay = Arc::new(|round| {
        if round > 16 {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(10)
        }
    });
    delay_config.coord_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.parent_request_delay = Arc::new(|_| Duration::from_millis(10));
    delay_config.newest_request_delay = Arc::new(|_| Duration::from_millis(10));

    FederationAlephBftLiveSessionOptions::new(pump_config)
        .with_max_round(max_round)
        .with_delay_config(delay_config)
}

async fn wait_for_aleph_bft_session(state: &MintState, session_id: u64) {
    time::timeout(Duration::from_secs(10), async {
        loop {
            let status = state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status");
            if status.session == session_id {
                return;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT session did not advance before timeout")
}

async fn operation_applied_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> bool {
    operation_terminal_within(state, operation_id, timeout).await
        == Some(FederationOperationStatus::Applied)
}

async fn operation_terminal_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> Option<FederationOperationStatus> {
    if let Some(entry) = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await
        .expect("operation status lookup")
    {
        if matches!(
            entry.status(),
            FederationOperationStatus::Applied | FederationOperationStatus::Rejected
        ) {
            return Some(entry.status());
        }
    }

    let response = time::timeout(
        timeout,
        state.wait_for_federation_operation_acceptance(operation_id),
    )
    .await
    .ok()
    .flatten()?;
    let status = response.outcome.entry().status();
    matches!(
        status,
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )
    .then_some(status)
}

async fn wait_for_aleph_bft_pending_operations(
    state: &MintState,
    session_id: u64,
) -> Vec<FederationAlephBftPendingOperationRecord> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let pending = state
                .mint
                .localstore()
                .get_federation_aleph_bft_pending_operations(session_id)
                .await
                .expect("AlephBFT pending operation lookup");
            if !pending.is_empty() {
                return pending;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT pending operations were not persisted")
}

async fn wait_for_aleph_bft_network_message_request(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    receiver: MemberId,
) -> FederationSignedRequest<FederationAlephBftNetworkMessageRequest> {
    time::timeout(
        Duration::from_secs(5),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender && request.receiver == receiver
        }),
    )
    .await
    .expect("AlephBFT network message request was not captured")
}

async fn wait_for_aleph_bft_unit_with_operation(
    transport: &RouterFederationTransportRegistry,
    sender: MemberId,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let request = time::timeout(
        Duration::from_secs(10),
        transport.wait_for_aleph_bft_network_message_request_matching(|request| {
            request.sender == sender
                && aleph_bft_network_request_contains_operation(request, operation_id)
        }),
    )
    .await
    .ok()?;
    aleph_bft_network_request_operation_unit_index(&request, operation_id)
}

fn aleph_bft_network_request_contains_operation(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> bool {
    aleph_bft_network_request_operation_unit_index(request, operation_id).is_some()
}

fn aleph_bft_network_request_operation_unit_index(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> Option<u64> {
    let bytes = request.payload.message_bytes().ok()?;
    let mut input = &bytes[..];
    let message = CdkAlephBftNetworkData::decode(&mut input).ok()?;
    if !input.is_empty() {
        return None;
    }
    message.included_data().iter().find_map(|data| {
        let payload = data.payload();
        payload
            .operation_ids
            .contains(&operation_id)
            .then_some(payload.unit_index)
    })
}

async fn wait_for_aleph_bft_backup_bytes(state: &MintState, session_id: u64) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if !backup_bytes.is_empty() {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes were not persisted")
}

async fn seed_aleph_bft_backup(state: &MintState, session_id: u64, backup_bytes: &[u8]) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT backup seed transaction");
    tx.append_federation_aleph_bft_backup_bytes(session_id, backup_bytes)
        .await
        .expect("seed AlephBFT backup bytes");
    tx.set_federation_aleph_bft_active_session(session_id)
        .await
        .expect("seed AlephBFT active session");
    tx.commit()
        .await
        .expect("commit AlephBFT backup seed transaction");
}

async fn wait_for_aleph_bft_backup_bytes_longer_than(
    state: &MintState,
    session_id: u64,
    previous_len: usize,
) -> Vec<u8> {
    time::timeout(Duration::from_secs(5), async {
        loop {
            let backup_bytes = state
                .mint
                .localstore()
                .get_federation_aleph_bft_backup_bytes(session_id)
                .await
                .expect("AlephBFT backup byte lookup")
                .unwrap_or_default();
            if backup_bytes.len() > previous_len {
                return backup_bytes;
            }
            time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AlephBFT backup bytes did not grow")
}

