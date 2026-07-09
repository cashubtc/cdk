use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cdk::federation::FederationOperationId;

/// Test-only stage marker for AlephBFT latency probes.
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

/// One test-only AlephBFT latency event.
#[derive(Debug, Clone)]
pub(crate) struct CdkAlephBftLatencyEvent {
    /// Stage that occurred.
    pub(crate) stage: CdkAlephBftLatencyStage,
    /// Operation IDs carried by the stage. Network-only protocol traffic may be empty.
    pub(crate) operation_ids: Vec<FederationOperationId>,
    /// Elapsed time since the scoped recorder was installed.
    pub(crate) elapsed: Duration,
}

#[derive(Debug)]
struct CdkAlephBftLatencyRecorderState {
    started_at: Instant,
    events: Vec<CdkAlephBftLatencyEvent>,
}

static ACTIVE_RECORDER: Mutex<Option<Arc<Mutex<CdkAlephBftLatencyRecorderState>>>> =
    Mutex::new(None);

/// Scoped test-only recorder for AlephBFT latency events.
#[derive(Debug)]
pub(crate) struct CdkAlephBftLatencyRecorder {
    inner: Arc<Mutex<CdkAlephBftLatencyRecorderState>>,
}

impl CdkAlephBftLatencyRecorder {
    /// Install a recorder until this guard is dropped.
    pub(crate) fn install() -> Self {
        let inner = Arc::new(Mutex::new(CdkAlephBftLatencyRecorderState {
            started_at: Instant::now(),
            events: Vec::new(),
        }));
        *ACTIVE_RECORDER
            .lock()
            .expect("AlephBFT latency recorder mutex poisoned") = Some(Arc::clone(&inner));

        Self { inner }
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
}

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

/// Record a test-only latency stage if a scoped recorder is installed.
pub(crate) fn record(
    stage: CdkAlephBftLatencyStage,
    operation_ids: impl IntoIterator<Item = FederationOperationId>,
) {
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
        operation_ids: operation_ids.into_iter().collect(),
        elapsed,
    });
}
