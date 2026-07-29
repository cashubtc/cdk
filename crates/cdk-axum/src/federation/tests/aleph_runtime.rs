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

fn aleph_bft_swap_benchmark_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"))
        })
        .unwrap_or(default)
}

fn aleph_bft_swap_benchmark_options(
    unit_interval_ms: u64,
    pump_messages_per_tick: usize,
) -> FederationAlephBftLiveSessionOptions {
    let mut delay_config = default_delay_config();
    let startup_delay = (delay_config.unit_creation_delay)(0);
    delay_config.unit_creation_delay = Arc::new(move |round| {
        if round == 0 {
            startup_delay
        } else {
            Duration::from_millis(unit_interval_ms)
        }
    });

    FederationAlephBftLiveSessionOptions::new(CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: pump_messages_per_tick,
        max_peer_deliveries_per_message: 64,
    })
    .with_delay_config(delay_config)
}

fn aleph_bft_swap_benchmark_percentile(sorted: &[f64], percentile: usize) -> f64 {
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

fn aleph_bft_swap_benchmark_contains_application_data(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
) -> bool {
    request
        .payload
        .message_bytes()
        .ok()
        .and_then(|bytes| {
            let mut input = &bytes[..];
            CdkAlephBftNetworkData::decode(&mut input)
                .ok()
                .filter(|_| input.is_empty())
        })
        .is_some_and(|message| !message.included_data().is_empty())
}

#[tokio::test]
#[ignore = "manual release-mode AlephBFT steady-state swap benchmark"]
async fn benchmark_aleph_bft_serial_swaps() {
    let member_count = aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_MEMBERS", 5);
    let signature_threshold =
        aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_SIGNATURE_THRESHOLD", 3);
    let default_sync_config = FederationPeerSyncConfig::default();
    let default_unit_interval_ms = usize::try_from(
        default_sync_config
            .aleph_bft_active_unit_interval
            .as_millis(),
    )
    .expect("default active unit interval fits usize milliseconds");
    let unit_interval_ms = aleph_bft_swap_benchmark_env(
        "CDK_ALEPH_BFT_BENCH_UNIT_INTERVAL_MS",
        default_unit_interval_ms,
    );
    let pump_messages_per_tick = aleph_bft_swap_benchmark_env(
        "CDK_ALEPH_BFT_BENCH_PUMP_MESSAGES_PER_TICK",
        default_sync_config.aleph_bft_network_pump_max_messages_per_tick,
    );
    let swap_count = aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_SWAPS", 10);
    let network_rtt_ms =
        aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_NETWORK_RTT_MS", 0);
    let network_jitter_percent =
        aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_NETWORK_JITTER_PERCENT", 20);
    let network_jitter_seed =
        aleph_bft_swap_benchmark_env("CDK_ALEPH_BFT_BENCH_NETWORK_JITTER_SEED", 1);
    assert!(member_count >= 4, "benchmark requires at least four members");
    assert!(
        signature_threshold > 0 && signature_threshold <= member_count,
        "signature threshold must fit the member count"
    );
    assert!(unit_interval_ms > 0, "unit interval must be positive");
    assert!(
        pump_messages_per_tick > 0,
        "pump messages per tick must be positive"
    );
    assert!(swap_count >= 10, "benchmark requires at least ten swaps");
    assert!(
        network_jitter_percent <= 100,
        "network jitter percent must not exceed 100"
    );

    let latency_reservation = CdkAlephBftLatencyRecorder::reserve().await;
    let startup_started = std::time::Instant::now();
    let session = start_multi_member_aleph_bft_live_session(
        u16::try_from(member_count).expect("member count fits u16"),
        u16::try_from(signature_threshold).expect("signature threshold fits u16"),
        aleph_bft_swap_benchmark_options(
            u64::try_from(unit_interval_ms).expect("unit interval fits u64"),
            pump_messages_per_tick,
        ),
    )
    .await
    .expect("start swap benchmark federation");
    for state in &session.states {
        state.restore_federation_startup_readiness().await;
    }

    let warmup_operation = submit_healthy_federation_swap_batch(&session, 0, 1, 1).await[0];
    for state in &session.states {
        wait_for_applied_operation_with_context(state, warmup_operation, "benchmark warmup").await;
    }
    let startup_ms = startup_started.elapsed().as_secs_f64() * 1_000.0;
    session.transport.set_aleph_bft_network_latency_model(
        Duration::from_millis(u64::try_from(network_rtt_ms).expect("network RTT fits u64")),
        u8::try_from(network_jitter_percent).expect("network jitter percent fits u8"),
        u64::try_from(network_jitter_seed).expect("network jitter seed fits u64"),
    );

    let request_start = session
        .transport
        .aleph_bft_network_message_requests()
        .len();
    let latency = latency_reservation.install();
    let mut operation_ids = Vec::with_capacity(swap_count);
    let mut operation_latencies_ms = Vec::with_capacity(swap_count);
    let steady_started = std::time::Instant::now();
    for index in 1..=swap_count {
        let operation_id =
            submit_healthy_federation_swap_batch(&session, index, 1, 1).await[0];
        for state in &session.states {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                "serial swap benchmark",
            )
            .await;
        }
        let events = latency.events_for_operation(operation_id);
        assert_latency_stage_order(&events, LATENCY_EXPECTED_STAGES);
        let elapsed = latency_stage_delta(
            &events,
            CdkAlephBftLatencyStage::SubmitQueued,
            CdkAlephBftLatencyStage::WaiterReturned,
        )
        .expect("serial swap submit-to-waiter latency");
        operation_ids.push(operation_id);
        operation_latencies_ms.push(elapsed.as_secs_f64() * 1_000.0);
    }
    let steady_state_ms = steady_started.elapsed().as_secs_f64() * 1_000.0;
    let request_end = session
        .transport
        .aleph_bft_network_message_requests()
        .len();
    let network = latency.network_stats();
    drop(latency);

    let mut expected_operation_ids = Vec::with_capacity(operation_ids.len() + 1);
    expected_operation_ids.push(warmup_operation);
    expected_operation_ids.extend(operation_ids.iter().copied());
    assert_multi_member_journal_and_state_converged(&session, &expected_operation_ids).await;
    for state in &session.states {
        assert_eq!(
            state
                .federation_sync_transition_stats()
                .await
                .expect("benchmark sync transition stats"),
            crate::FederationSyncTransitionStats::default(),
            "healthy benchmark member must not enter lagging or catch-up"
        );
    }

    let requests = session.transport.aleph_bft_network_message_requests();
    let measured_requests = &requests[request_start..request_end];
    let application_messages = measured_requests
        .iter()
        .filter(|request| aleph_bft_swap_benchmark_contains_application_data(request))
        .count();
    let total_messages = measured_requests.len();
    let helper_messages = total_messages.saturating_sub(application_messages);
    let delivery_bytes = measured_requests
        .iter()
        .map(|request| {
            request
                .payload
                .ensure_admissible_size()
                .expect("benchmark network message size")
        })
        .sum::<usize>();
    let request_body_bytes = measured_requests
        .iter()
        .map(|request| {
            serde_json::to_vec(request)
                .expect("serialize benchmark network request")
                .len()
        })
        .sum::<usize>();
    let steady_state_seconds = steady_state_ms / 1_000.0;
    let request_body_kbit_per_second =
        request_body_bytes as f64 * 8.0 / 1_000.0 / steady_state_seconds;

    let mut sorted = operation_latencies_ms.clone();
    sorted.sort_by(f64::total_cmp);
    let median_ms = if sorted.len() % 2 == 0 {
        let right = sorted.len() / 2;
        (sorted[right - 1] + sorted[right]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };
    let result = serde_json::json!({
        "members": member_count,
        "signature_threshold": signature_threshold,
        "unit_interval_ms": unit_interval_ms,
        "pump_messages_per_tick": pump_messages_per_tick,
        "network_rtt_ms": network_rtt_ms,
        "network_jitter_percent": network_jitter_percent,
        "network_jitter_seed": network_jitter_seed,
        "swaps": swap_count,
        "startup_ms": startup_ms,
        "steady_state_ms": steady_state_ms,
        "latencies_ms": operation_latencies_ms,
        "min_ms": sorted[0],
        "median_ms": median_ms,
        "p95_ms": aleph_bft_swap_benchmark_percentile(&sorted, 95),
        "max_ms": sorted[sorted.len() - 1],
        "total_deliveries": total_messages,
        "application_deliveries": application_messages,
        "helper_deliveries": helper_messages,
        "delivery_bytes": delivery_bytes,
        "request_body_bytes": request_body_bytes,
        "request_body_kbit_per_second": request_body_kbit_per_second,
        "outbound_messages_enqueued": network.outbound_messages_enqueued,
        "outbound_messages_dropped": network.outbound_messages_dropped,
        "max_outbound_queue_depth": network.max_outbound_queue_depth,
        "pump_ticks": network.pump_ticks,
        "messages_drained": network.messages_drained,
        "encoded_bytes_drained": network.encoded_bytes_drained,
        "retry_messages_drained": network.retry_messages_drained,
        "max_retry_queue_depth": network.max_retry_queue_depth,
        "retry_deliveries_dropped": network.retry_deliveries_dropped,
        "backlogged_pump_ticks": network.backlogged_pump_ticks,
    });
    println!(
        "ALEPH_BFT_SWAP_BENCH {}",
        serde_json::to_string(&result).expect("serialize swap benchmark result")
    );
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
