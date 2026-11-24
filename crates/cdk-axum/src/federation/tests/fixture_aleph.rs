//! AlephBFT waits, latency probes and synthetic checkpoint history.

use super::*;

pub(super) async fn queue_local_aleph_bft_unit(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationAlephBftUnitPayload {
    let consensus = state.federation.consensus().expect("consensus");
    let mut consensus = consensus.lock().await;
    consensus
        .submit_aleph_bft_envelope(envelope)
        .expect("queue AlephBFT envelope");
    consensus
        .next_aleph_bft_unit_payload()
        .expect("drain AlephBFT unit")
        .expect("queued unit")
}

pub(super) async fn wait_for_local_aleph_bft_unit(
    state: &MintState,
) -> FederationAlephBftUnitPayload {
    for _ in 0..50 {
        let maybe_unit = {
            let consensus = state.federation.consensus().expect("consensus");
            let mut consensus = consensus.lock().await;
            consensus
                .next_aleph_bft_unit_payload()
                .expect("drain AlephBFT unit")
        };
        if let Some(unit) = maybe_unit {
            return unit;
        }
        tokio::task::yield_now().await;
    }

    panic!("AlephBFT unit was not queued");
}

pub(super) fn assert_pending_finalization_error(
    err: &cdk::Error,
    operation_id: FederationOperationId,
    timeout_ms: u64,
) {
    assert!(
        !err.is_definitive_failure(),
        "pending finalization must remain retryable: {err}"
    );
    match err {
        cdk::Error::Federation(FederationError::OperationPendingFinalization {
            operation_id: found_operation_id,
            timeout_ms: found_timeout_ms,
        }) => {
            assert_eq!(found_operation_id, &operation_id);
            assert_eq!(*found_timeout_ms, timeout_ms);
        }
        _ => panic!("unexpected error: {err}"),
    }
}

pub(super) async fn submit_latency_probe_mint_quote(state: &MintState) -> FederationOperationId {
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

pub(super) async fn submit_duplicate_latency_probe_mint_quote(
    state: &MintState,
) -> FederationOperationId {
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

pub(super) async fn wait_for_latency_stage(
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

pub(super) fn assert_latency_stage_order(
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

pub(super) fn latency_stage_delta(
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

pub(super) fn latency_stage_count(
    events: &[CdkAlephBftLatencyEvent],
    stage: CdkAlephBftLatencyStage,
) -> usize {
    events.iter().filter(|event| event.stage == stage).count()
}

pub(super) fn format_latency_events(events: &[CdkAlephBftLatencyEvent]) -> String {
    events
        .iter()
        .map(|event| format!("{:?}@{}ms", event.stage, event.elapsed.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn fast_aleph_bft_live_session_options(
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

pub(super) fn fast_aleph_bft_live_session_options_with_idle_tail(
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

pub(super) async fn wait_for_aleph_bft_session(state: &MintState, session_id: u64) {
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

pub(super) async fn operation_applied_within(
    state: &MintState,
    operation_id: FederationOperationId,
    timeout: Duration,
) -> bool {
    operation_terminal_within(state, operation_id, timeout).await
        == Some(FederationOperationStatus::Applied)
}

pub(super) async fn operation_terminal_within(
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

pub(super) async fn wait_for_aleph_bft_pending_operations(
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

pub(super) async fn wait_for_aleph_bft_network_message_request(
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

pub(super) async fn wait_for_aleph_bft_unit_with_operation(
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

pub(super) fn aleph_bft_network_request_contains_operation(
    request: &FederationSignedRequest<FederationAlephBftNetworkMessageRequest>,
    operation_id: FederationOperationId,
) -> bool {
    aleph_bft_network_request_operation_unit_index(request, operation_id).is_some()
}

pub(super) fn aleph_bft_network_request_operation_unit_index(
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

pub(super) async fn wait_for_aleph_bft_backup_bytes(state: &MintState, session_id: u64) -> Vec<u8> {
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

pub(super) async fn seed_aleph_bft_backup(state: &MintState, session_id: u64, backup_bytes: &[u8]) {
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

pub(super) async fn wait_for_aleph_bft_backup_bytes_longer_than(
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

#[cfg(feature = "prometheus")]
pub(super) fn synthetic_conflicting_mint_quote_envelope(
    base: &FederationOperationEnvelope,
    private_config: &MemberConfigPrivate,
    operation_index: u64,
) -> FederationOperationEnvelope {
    let mut envelope = base.clone();
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("synthetic history base must be a mint quote");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("synthetic history base must use a bolt11 mint quote");
    };
    response
        .request
        .push_str(&format!("-synthetic-conflict-{operation_index}"));
    envelope
        .authorize(private_config)
        .expect("authorize synthetic conflicting operation")
}

#[cfg(feature = "prometheus")]
pub(super) async fn reset_synthetic_history_consensus(
    state: &mut MintState,
    public_config: &FederationConfig,
    base_entry: &FederationJournalEntry,
) {
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("synthetic history checkpoint frontier");
    let (status, next_unit_index_by_member) = {
        let consensus = state
            .federation
            .consensus()
            .expect("synthetic history consensus")
            .lock()
            .await;
        (
            consensus.status(),
            consensus.next_unit_index_by_member().clone(),
        )
    };
    let boundary = FederationCheckpointBoundary::from_complete_unit(
        &frontier,
        status.session,
        status.next_unit_finalization_index,
        next_unit_index_by_member,
    )
    .expect("synthetic history complete-unit boundary");
    let checkpoint = boundary
        .recovery_checkpoint(
            public_config,
            None,
            FederationMaterializedStateDigest::genesis(),
        )
        .expect("synthetic history transient checkpoint");
    let mut consensus = FederationConsensusRuntime::checkpoint_base_for_test(
        public_config,
        status.local_member,
        &checkpoint,
    )
    .expect("reset synthetic history consensus");
    consensus
        .register_historical_journal_entry(base_entry.clone())
        .expect("register synthetic history conflict anchor");
    state.federation.member_mut_for_test().consensus = Arc::new(tokio::sync::Mutex::new(consensus));
}

#[cfg(feature = "prometheus")]
pub(super) async fn append_synthetic_history_conflicts(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    base_envelope: &FederationOperationEnvelope,
    base_entry: &FederationJournalEntry,
    mut next_operation_index: u64,
    operation_count: u64,
) {
    while next_operation_index < operation_count {
        let batch_len = usize::try_from(
            operation_count
                .saturating_sub(next_operation_index)
                .min(cdk::federation::FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS as u64),
        )
        .expect("synthetic batch length fits in usize");
        let items = (0..batch_len)
            .map(|offset| {
                let operation_index = next_operation_index.saturating_add(
                    u64::try_from(offset).expect("synthetic batch offset fits in u64"),
                );
                FederationConsensusItem::operation(synthetic_conflicting_mint_quote_envelope(
                    base_envelope,
                    local_private,
                    operation_index,
                ))
            })
            .collect::<Vec<_>>();
        let (finalized_items, entries) = {
            let mut consensus = state
                .federation
                .consensus()
                .expect("synthetic history consensus")
                .lock()
                .await;
            let status = consensus.status();
            let payload = FederationAlephBftUnitPayload::new(
                status.federation_id,
                status.local_member,
                status.session,
                consensus.next_unit_index(),
                items,
            )
            .expect("synthetic history finalized payload");
            let finalized_start = consensus.finalized_items().len();
            let responses = consensus
                .finalize_aleph_bft_unit(payload)
                .expect("finalize synthetic history batch");
            let finalized_items = consensus.finalized_items()[finalized_start..].to_vec();
            let entries = responses
                .into_iter()
                .map(|response| match response.outcome {
                    FederationJournalAppendOutcome::Appended(entry) => entry,
                    FederationJournalAppendOutcome::Duplicate(_) => {
                        panic!("synthetic history operation must be unique")
                    }
                })
                .collect::<Vec<_>>();
            (finalized_items, entries)
        };
        assert_eq!(finalized_items.len(), batch_len);
        assert_eq!(entries.len(), batch_len);
        assert!(entries.iter().all(|entry| {
            matches!(
                entry.application,
                FederationApplicationState::ConsensusRejected {}
            )
        }));
        state
            .federation
            .persist_finalization(&finalized_items, &entries)
            .await
            .expect("persist synthetic history batch");
        next_operation_index = next_operation_index
            .saturating_add(u64::try_from(batch_len).expect("synthetic batch length fits in u64"));
        reset_synthetic_history_consensus(state, public_config, base_entry).await;
    }
}

#[cfg(feature = "prometheus")]
pub(super) async fn build_synthetic_checkpoint_history(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    peer_private: &MemberConfigPrivate,
    operation_count: u64,
) -> FederationCheckpoint {
    assert!(operation_count > 0, "synthetic history must be non-empty");
    let quote_id = QuoteId::new();
    let base_envelope = mint_quote_envelope(state, &quote_id);
    state
        .submit_federation_envelope(base_envelope.clone())
        .await
        .expect("apply synthetic history conflict anchor");
    let base_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(
            base_envelope.operation_id().expect("base operation id"),
        )
        .await
        .expect("load synthetic history conflict anchor")
        .expect("synthetic history conflict anchor exists");

    append_synthetic_history_conflicts(
        state,
        public_config,
        local_private,
        &base_envelope,
        &base_entry,
        1,
        operation_count,
    )
    .await;

    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("completed synthetic history frontier");
    assert_eq!(frontier.journal_next_index, operation_count);
    assert_eq!(frontier.application_next_index, operation_count);
    assert_eq!(frontier.finalized_next_item_index, operation_count);

    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        state,
        public_config,
        None,
        &[local_private, peer_private],
    )
    .await;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(public_config)
            .expect("verify synthetic checkpoint quorum"),
    )
    .await
    .expect("promote synthetic checkpoint");
    tx.commit()
        .await
        .expect("commit synthetic checkpoint promotion");
    checkpoint
}

#[cfg(feature = "prometheus")]
pub(super) fn checkpoint_work_delta(member: &str, work: &str, before: f64) -> f64 {
    prometheus_metric_value(
        "cdk_federation_checkpoint_work_total",
        &[("member", member), ("work", work)],
    ) - before
}

pub(super) async fn seed_synthetic_active_session_backup(
    state: &MintState,
    session: u64,
    chunk_count: usize,
    bytes_per_chunk: usize,
) {
    let chunks = (0..chunk_count)
        .map(|index| {
            vec![u8::try_from(index.saturating_add(1)).unwrap_or(u8::MAX); bytes_per_chunk]
        })
        .collect::<Vec<_>>();
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic backup transaction");
    tx.replace_federation_aleph_bft_backup_chunks(session, &chunks)
        .await
        .expect("persist synthetic backup chunks");
    tx.set_federation_aleph_bft_active_session(session)
        .await
        .expect("persist synthetic active session");
    tx.commit().await.expect("commit synthetic backup");
}

pub(super) async fn wait_for_applied_operation(
    state: &MintState,
    operation_id: FederationOperationId,
) {
    assert!(
        operation_applied_within(state, operation_id, Duration::from_secs(15)).await,
        "operation applied before timeout"
    );
}

pub(super) fn journal_convergence_projection(
    entries: &[FederationJournalEntry],
) -> Vec<(
    u64,
    FederationOperationId,
    FederationJournalDigest,
    FederationJournalDigest,
    FederationOperationStatus,
)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.index,
                entry.operation_id,
                entry.previous_order_digest,
                entry.order_digest,
                entry.status(),
            )
        })
        .collect()
}

pub(super) async fn wait_for_terminal_operation(
    state: &MintState,
    operation_id: FederationOperationId,
) -> FederationOperationStatus {
    operation_terminal_within(state, operation_id, Duration::from_secs(30))
        .await
        .expect("operation reached terminal status before timeout")
}

pub(super) async fn replay_aleph_bft_runtime_after_test_process_restart(state: &mut MintState) {
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("replay AlephBFT runtime after test process restart");
    state.federation.member_mut_for_test().consensus =
        replayed_consensus.expect("configured member handle");
    state.federation.member_mut_for_test().sequences =
        replayed_sequences.expect("configured member handle");
}

pub(super) async fn submit_live_operation_allowing_pending_timeout(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationOperationId {
    let operation_id = envelope.operation_id().expect("live operation id");
    match state
        .submit_federation_envelope_to_consensus(envelope)
        .await
    {
        Ok(response) => assert_eq!(response.operation_id, operation_id),
        Err(err) => assert_pending_finalization_error(&err, operation_id, 10_000),
    }

    operation_id
}

pub(super) async fn wait_for_applied_operation_with_context(
    state: &MintState,
    operation_id: FederationOperationId,
    context: &str,
) {
    if operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
        return;
    }

    let member_id = state
        .mint
        .federation_config()
        .expect("contextual apply wait federation config")
        .member_id();
    let status = state.federation_aleph_bft_status().await;
    let journal_operation_ids = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("contextual apply wait journal")
        .into_iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let pending_operation_ids = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("contextual apply wait pending operations")
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect::<Vec<_>>();
    panic!(
        "{context}: operation {operation_id} did not apply on member {member_id} before timeout; status={status:?}; journal_operation_ids={journal_operation_ids:?}; pending_operation_ids={pending_operation_ids:?}"
    );
}

pub(super) async fn sync_and_certify_partitioned_member(
    session: &MultiMemberAlephBftLiveSession,
    partitioned_index: usize,
    recovery_live_options: FederationAlephBftLiveSessionOptions,
) {
    let state = &session.states[partitioned_index];
    let public_config = &session.public_config;
    let clients = session
        .private_configs
        .iter()
        .cloned()
        .map(|private_config| {
            FederationRestPeerClient::new_with_config(
                session.transport.clone(),
                public_config,
                private_config,
                session.sync_config,
            )
            .expect("partition recovery peer-sync client")
        })
        .collect::<Vec<_>>();
    let peer_sets = session
        .private_configs
        .iter()
        .map(|private_config| {
            public_config
                .members
                .values()
                .filter(|member| member.member_id != private_config.member_id)
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let required_remote_quorum =
        usize::from(public_config.threshold_params.consensus_threshold) - 1;
    let catch_up_deadline = time::Instant::now() + Duration::from_secs(90);
    let mut maximum_synced = 0;
    let mut restarted_with_fast_test_options = false;
    loop {
        for index in 0..session.states.len() {
            let synced = run_federation_peer_sync_round(
                &session.states[index],
                &clients[index],
                &peer_sets[index],
            )
            .await;
            if index == partitioned_index {
                maximum_synced = maximum_synced.max(synced);
            }
        }
        if !restarted_with_fast_test_options
            && state.federation.live_session_active().await
            && matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::CatchingUp { .. })
            )
        {
            let local_status = local_peer_status(state)
                .await
                .expect("partition recovery local status");
            let mut peer_statuses = Vec::new();
            for peer in &peer_sets[partitioned_index] {
                peer_statuses.push(
                    clients[partitioned_index]
                        .peer_status(peer)
                        .await
                        .expect("partition recovery peer status"),
                );
            }
            let target_session = sync_loop::quorum_compatible_session(
                &local_status,
                &peer_statuses,
                usize::from(public_config.threshold_params.consensus_threshold),
            )
            .expect("partition recovery quorum-compatible session");
            let replay_target = sync_loop::quorum_supported_replay_target(
                &local_status,
                &peer_statuses,
                target_session,
                usize::from(public_config.threshold_params.consensus_threshold),
            );
            stop_current_federation_aleph_bft_session_for_catch_up(state).await;
            start_federation_aleph_bft_live_session_with_options_and_transport(
                state,
                session.sync_config,
                recovery_live_options
                    .clone()
                    .with_replay_target_next_unit_index_by_member(replay_target),
                session.transport.clone(),
            )
            .await
            .expect("restart partition recovery with fast local test options")
            .expect("partition recovery fast live session");
            restarted_with_fast_test_options = true;
        }
        match state.ensure_federation_ready_for_authoritative_read().await {
            Ok(()) => {
                assert!(
                    maximum_synced >= required_remote_quorum,
                    "catch-up certification must reach a remote BFT quorum"
                );
                return;
            }
            Err(_) if time::Instant::now() < catch_up_deadline => {
                time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => {
                let local_status = local_peer_status(state)
                    .await
                    .expect("partition recovery timeout status");
                let trusted_checkpoint = state
                    .mint
                    .localstore()
                    .get_trusted_federation_checkpoint_quorum()
                    .await
                    .expect("partition recovery timeout checkpoint")
                    .map(|quorum| quorum.checkpoint.id().expect("trusted checkpoint id"));
                panic!(
                    "partitioned member did not become ready after ordered recovery; maximum synced peers: {maximum_synced}; required remote quorum: {required_remote_quorum}; readiness error: {err}; local_status={local_status:?}; live_session_active={}; replay_ready={}; trusted_checkpoint={trusted_checkpoint:?}",
                    state.federation.live_session_active().await,
                    state.federation.live_session_replay_ready().await,
                )
            }
        }
    }
}

pub(super) const LATENCY_EXPECTED_STAGES: &[CdkAlephBftLatencyStage] = &[
    CdkAlephBftLatencyStage::SubmitQueued,
    CdkAlephBftLatencyStage::DataProviderYielded,
    CdkAlephBftLatencyStage::NetworkMessageDelivered,
    CdkAlephBftLatencyStage::FinalizedDataReceived,
    CdkAlephBftLatencyStage::ApplyLoopReceived,
    CdkAlephBftLatencyStage::ApplyCompleted,
    CdkAlephBftLatencyStage::OperationNotified,
    CdkAlephBftLatencyStage::WaiterReturned,
];
