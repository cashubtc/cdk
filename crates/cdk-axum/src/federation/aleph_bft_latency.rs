#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(test)]
use std::time::{Duration, Instant};

use cdk::federation::FederationOperationId;
#[cfg(test)]
use tokio::sync::OwnedMutexGuard;

/// Stage marker for opt-in AlephBFT operation lifecycle diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CdkAlephBftLatencyStage {
    /// A public or private operation was admitted into the AlephBFT mempool.
    SubmitQueued,
    /// The upstream AlephBFT data provider drained CDK consensus data.
    DataProviderYielded,
    /// A private REST AlephBFT network message was accepted by a peer.
    NetworkMessageDelivered,
    /// Upstream AlephBFT finalized CDK consensus data.
    FinalizedDataReceived,
    /// The finalized-unit apply loop received a finalized CDK unit.
    ApplyLoopReceived,
    /// The finalized CDK unit was durably applied before waiter notification.
    ApplyCompleted,
    /// The operation waiter notification cache received the accepted response.
    OperationNotified,
    /// The public submit waiter returned to the caller.
    WaiterReturned,
}

impl CdkAlephBftLatencyStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::SubmitQueued => "submit_queued",
            Self::DataProviderYielded => "data_provider_yielded",
            Self::NetworkMessageDelivered => "network_message_delivered",
            Self::FinalizedDataReceived => "finalized_data_received",
            Self::ApplyLoopReceived => "apply_loop_received",
            Self::ApplyCompleted => "apply_completed",
            Self::OperationNotified => "operation_notified",
            Self::WaiterReturned => "waiter_returned",
        }
    }
}

/// One test-only AlephBFT latency event.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct CdkAlephBftLatencyEvent {
    /// Stage that occurred.
    pub(crate) stage: CdkAlephBftLatencyStage,
    /// Operation IDs carried by the stage. Network-only protocol traffic may be empty.
    pub(crate) operation_ids: Vec<FederationOperationId>,
    /// Elapsed time since the scoped recorder was installed.
    pub(crate) elapsed: Duration,
}

/// Test-only network-pump counters captured in the same scoped window.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CdkAlephBftNetworkProbeStats {
    /// Outbound messages successfully queued by AlephBFT.
    pub(crate) outbound_messages_enqueued: usize,
    /// Outbound messages dropped because the bounded bridge was full.
    pub(crate) outbound_messages_dropped: usize,
    /// Largest observed outbound bridge depth.
    pub(crate) max_outbound_queue_depth: usize,
    /// Network-pump ticks that performed work.
    pub(crate) pump_ticks: usize,
    /// Fresh outbound messages drained by the pump.
    pub(crate) messages_drained: usize,
    /// Encoded bytes drained before per-peer fan-out.
    pub(crate) encoded_bytes_drained: usize,
    /// Retry messages drained by the pump.
    pub(crate) retry_messages_drained: usize,
    /// Largest retry queue observed after a pump tick.
    pub(crate) max_retry_queue_depth: usize,
    /// Retry deliveries evicted from the bounded retry queue.
    pub(crate) retry_deliveries_dropped: usize,
    /// Pump ticks that left fresh outbound messages queued.
    pub(crate) backlogged_pump_ticks: usize,
}

#[cfg(test)]
#[derive(Debug)]
struct CdkAlephBftLatencyRecorderState {
    started_at: Instant,
    events: Vec<CdkAlephBftLatencyEvent>,
    network: CdkAlephBftNetworkProbeStats,
    changed: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
static ACTIVE_RECORDER: Mutex<Option<Arc<Mutex<CdkAlephBftLatencyRecorderState>>>> =
    Mutex::new(None);
#[cfg(test)]
static INSTALL_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

/// Scoped test-only recorder for AlephBFT latency events.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct CdkAlephBftLatencyRecorder {
    inner: Arc<Mutex<CdkAlephBftLatencyRecorderState>>,
    _install_guard: OwnedMutexGuard<()>,
}

/// Exclusive reservation for a latency probe that has not started recording yet.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct CdkAlephBftLatencyRecorderReservation {
    install_guard: OwnedMutexGuard<()>,
}

#[cfg(test)]
impl CdkAlephBftLatencyRecorder {
    /// Reserve exclusive access for a latency probe without recording setup traffic.
    pub(crate) async fn reserve() -> CdkAlephBftLatencyRecorderReservation {
        let install_lock = INSTALL_LOCK
            .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        CdkAlephBftLatencyRecorderReservation {
            install_guard: install_lock.lock_owned().await,
        }
    }

    fn install_with_guard(install_guard: OwnedMutexGuard<()>) -> Self {
        let inner = Arc::new(Mutex::new(CdkAlephBftLatencyRecorderState {
            started_at: Instant::now(),
            events: Vec::new(),
            network: CdkAlephBftNetworkProbeStats::default(),
            changed: Arc::new(tokio::sync::Notify::new()),
        }));
        *ACTIVE_RECORDER
            .lock()
            .expect("AlephBFT latency recorder mutex poisoned") = Some(Arc::clone(&inner));

        Self {
            inner,
            _install_guard: install_guard,
        }
    }

    /// Return all recorded events.
    pub(crate) fn events(&self) -> Vec<CdkAlephBftLatencyEvent> {
        self.inner
            .lock()
            .expect("AlephBFT latency recorder state mutex poisoned")
            .events
            .clone()
    }

    /// Return events carrying a specific operation ID.
    pub(crate) fn events_for_operation(
        &self,
        operation_id: FederationOperationId,
    ) -> Vec<CdkAlephBftLatencyEvent> {
        self.events()
            .into_iter()
            .filter(|event| event.operation_ids.contains(&operation_id))
            .collect()
    }

    /// Return the network-pump counters captured in this recorder's window.
    pub(crate) fn network_stats(&self) -> CdkAlephBftNetworkProbeStats {
        self.inner
            .lock()
            .expect("AlephBFT latency recorder state mutex poisoned")
            .network
    }

    /// Wait until a stage is recorded for an operation.
    pub(crate) async fn wait_for_stage(
        &self,
        operation_id: FederationOperationId,
        stage: CdkAlephBftLatencyStage,
    ) {
        loop {
            let state = self
                .inner
                .lock()
                .expect("AlephBFT latency recorder state mutex poisoned");
            let changed = state.changed.clone().notified_owned();
            if state
                .events
                .iter()
                .any(|event| event.stage == stage && event.operation_ids.contains(&operation_id))
            {
                return;
            }
            drop(state);
            changed.await;
        }
    }
}

#[cfg(test)]
impl CdkAlephBftLatencyRecorderReservation {
    /// Start recording after the probe's live-session setup is complete.
    pub(crate) fn install(self) -> CdkAlephBftLatencyRecorder {
        CdkAlephBftLatencyRecorder::install_with_guard(self.install_guard)
    }
}

#[cfg(test)]
impl Drop for CdkAlephBftLatencyRecorder {
    fn drop(&mut self) {
        let mut active = ACTIVE_RECORDER
            .lock()
            .expect("AlephBFT latency recorder mutex poisoned");
        if active
            .as_ref()
            .map(|inner| Arc::ptr_eq(inner, &self.inner))
            .unwrap_or_default()
        {
            *active = None;
        }
    }
}

/// Trace an operation lifecycle stage and record it when a test probe is installed.
pub(crate) fn record(
    stage: CdkAlephBftLatencyStage,
    operation_ids: impl IntoIterator<Item = FederationOperationId>,
) {
    #[cfg(not(test))]
    for operation_id in operation_ids {
        tracing::trace!(
            target: "cdk_axum::federation::aleph_bft_latency",
            operation_id = %operation_id,
            stage = stage.as_str(),
            "AlephBFT operation lifecycle stage"
        );
    }

    #[cfg(test)]
    {
        let operation_ids = operation_ids.into_iter().collect::<Vec<_>>();
        for operation_id in &operation_ids {
            tracing::trace!(
                target: "cdk_axum::federation::aleph_bft_latency",
                operation_id = %operation_id,
                stage = stage.as_str(),
                "AlephBFT operation lifecycle stage"
            );
        }
        record_for_test(stage, operation_ids);
    }
}

#[cfg(test)]
fn record_for_test(stage: CdkAlephBftLatencyStage, operation_ids: Vec<FederationOperationId>) {
    let Some(inner) = ACTIVE_RECORDER
        .lock()
        .expect("AlephBFT latency recorder mutex poisoned")
        .clone()
    else {
        return;
    };
    let mut state = inner
        .lock()
        .expect("AlephBFT latency recorder state mutex poisoned");
    let elapsed = state.started_at.elapsed();
    state.events.push(CdkAlephBftLatencyEvent {
        stage,
        operation_ids,
        elapsed,
    });
    state.changed.notify_waiters();
}

/// Record one successfully queued outbound AlephBFT message.
#[cfg(test)]
pub(crate) fn record_network_outbound_enqueued(queue_depth: usize) {
    with_active_recorder(|state| {
        state.network.outbound_messages_enqueued =
            state.network.outbound_messages_enqueued.saturating_add(1);
        state.network.max_outbound_queue_depth =
            state.network.max_outbound_queue_depth.max(queue_depth);
    });
}

/// Record one outbound AlephBFT message dropped by bounded backpressure.
#[cfg(test)]
pub(crate) fn record_network_outbound_dropped() {
    with_active_recorder(|state| {
        state.network.outbound_messages_dropped =
            state.network.outbound_messages_dropped.saturating_add(1);
    });
}

/// Record one completed network-pump tick.
#[cfg(test)]
pub(crate) fn record_network_pump_tick(
    messages_drained: usize,
    encoded_bytes: usize,
    retry_messages_drained: usize,
    retry_queue_depth: usize,
    retry_deliveries_dropped: usize,
    outbound_queue_depth: usize,
) {
    if messages_drained == 0 && retry_messages_drained == 0 {
        return;
    }
    with_active_recorder(|state| {
        state.network.pump_ticks = state.network.pump_ticks.saturating_add(1);
        state.network.messages_drained = state
            .network
            .messages_drained
            .saturating_add(messages_drained);
        state.network.encoded_bytes_drained = state
            .network
            .encoded_bytes_drained
            .saturating_add(encoded_bytes);
        state.network.retry_messages_drained = state
            .network
            .retry_messages_drained
            .saturating_add(retry_messages_drained);
        state.network.max_retry_queue_depth =
            state.network.max_retry_queue_depth.max(retry_queue_depth);
        state.network.retry_deliveries_dropped = state
            .network
            .retry_deliveries_dropped
            .saturating_add(retry_deliveries_dropped);
        if outbound_queue_depth > 0 {
            state.network.backlogged_pump_ticks =
                state.network.backlogged_pump_ticks.saturating_add(1);
        }
    });
}

#[cfg(test)]
fn with_active_recorder(update: impl FnOnce(&mut CdkAlephBftLatencyRecorderState)) {
    let Some(inner) = ACTIVE_RECORDER
        .lock()
        .expect("AlephBFT latency recorder mutex poisoned")
        .clone()
    else {
        return;
    };
    let mut state = inner
        .lock()
        .expect("AlephBFT latency recorder state mutex poisoned");
    update(&mut state);
}
