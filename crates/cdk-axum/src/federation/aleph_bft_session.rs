use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::{
    create_config, default_delay_config, run_session, Config as AlephBftConfig, DelayConfig, Index,
    NodeCount, Terminator,
};
use cdk::federation::{
    FederationAlephBftActivityRequest, FederationError, FederationOperationId, MemberConfigPublic,
    MemberId,
};
use cdk_common::database::DynMintDatabase;
use cdk_http_client::{Async, Transport};
use futures::channel::oneshot;
use futures::future::join_all;
use futures::FutureExt;
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::MintState;

use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound,
    CdkAlephBftNetworkInboundHandle, CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
};
use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftFinalizedUnit, CdkAlephBftKeychain, CdkAlephBftNetwork,
    CdkAlephBftNetworkData, CdkAlephBftReplayReadiness, CdkAlephBftSessionIo,
    CdkAlephBftTokioSpawnHandle, CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE,
};
use super::peer_client::FederationRestPeerClient;
use super::sync_config::{federation_peer_sync_interval, FederationPeerSyncConfig};
use crate::{
    is_retryable_federation_apply_error, FEDERATION_APPLY_RETRY_INITIAL_DELAY,
    FEDERATION_APPLY_RETRY_MAX_DELAY,
};

// AlephBFT validates its default delay schedule by summing all configured
// round delays. Keep production live sessions comfortably bounded; natural
// completion now rotates into the next session, while longer process-level
// checkpoint windows remain separate coverage.
const CDK_ALEPH_BFT_MAX_ROUND: u16 = 3000;
const CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND: usize = 64;
const CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY: Duration = Duration::from_secs(2);
const CDK_ALEPH_BFT_ESSENTIAL_TASK_ROOT_EXIT_GRACE: Duration = Duration::from_secs(1);
static NEXT_ALEPH_BFT_SESSION_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_aleph_bft_session_generation() -> Result<u64, cdk::Error> {
    NEXT_ALEPH_BFT_SESSION_GENERATION
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |generation| {
            generation.checked_add(1)
        })
        .map_err(|_| cdk::Error::Custom("AlephBFT session generation overflowed".to_string()))
}

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationAlephBftLiveSessionOptions {
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
    time_to_reach_max_round: Duration,
    delay_config: Option<DelayConfig>,
    automatic_rotation_limit: Option<usize>,
    replay_target_next_unit_index_by_member: BTreeMap<MemberId, u64>,
    #[cfg(test)]
    panic_session_task: bool,
    #[cfg(test)]
    fail_before_task_spawn: bool,
    #[cfg(test)]
    fail_next_finalized_payload_apply: bool,
    #[cfg(test)]
    panic_network_pump_task: bool,
    #[cfg(test)]
    fail_automatic_rotation_start: bool,
    #[cfg(test)]
    publication_pause: Option<(Arc<Notify>, Arc<Notify>)>,
    #[cfg(test)]
    root_task_activation_observer: Option<Arc<AtomicBool>>,
}

impl FederationAlephBftLiveSessionOptions {
    pub(crate) fn new(pump_config: CdkAlephBftNetworkPumpConfig) -> Self {
        Self {
            pump_config,
            ..Self::default()
        }
    }

    pub(crate) fn from_sync_config(
        pump_config: CdkAlephBftNetworkPumpConfig,
        sync_config: FederationPeerSyncConfig,
    ) -> Self {
        let mut options = Self::new(pump_config);
        if let Some(max_round) = sync_config.aleph_bft_live_session_max_round {
            let rotation_limit = sync_config
                .aleph_bft_live_session_rotation_limit
                .unwrap_or(1);
            options = options
                .with_max_round(max_round)
                .with_automatic_rotation_limit(rotation_limit);
        }
        options
    }

    pub(crate) fn with_max_round(mut self, max_round: u16) -> Self {
        self.max_round = max_round;
        self
    }

    #[cfg(test)]
    pub(crate) fn max_round(&self) -> u16 {
        self.max_round
    }

    #[cfg(test)]
    pub(crate) fn with_delay_config(mut self, delay_config: DelayConfig) -> Self {
        self.delay_config = Some(delay_config);
        self
    }

    pub(crate) fn with_automatic_rotation_limit(mut self, limit: usize) -> Self {
        self.automatic_rotation_limit = Some(limit);
        self
    }

    pub(crate) fn with_replay_target_next_unit_index_by_member(
        mut self,
        replay_target_next_unit_index_by_member: BTreeMap<MemberId, u64>,
    ) -> Self {
        self.replay_target_next_unit_index_by_member = replay_target_next_unit_index_by_member;
        self
    }

    #[cfg(test)]
    pub(crate) fn automatic_rotation_limit(&self) -> Option<usize> {
        self.automatic_rotation_limit
    }

    #[cfg(test)]
    pub(crate) fn with_panicking_session_task(mut self) -> Self {
        self.panic_session_task = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_publication_pause(
        mut self,
        reached: Arc<Notify>,
        release: Arc<Notify>,
    ) -> Self {
        self.publication_pause = Some((reached, release));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_root_task_activation_observer(mut self, observer: Arc<AtomicBool>) -> Self {
        self.root_task_activation_observer = Some(observer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_pre_spawn_failure(mut self) -> Self {
        self.fail_before_task_spawn = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_finalized_payload_apply_failure(mut self) -> Self {
        self.fail_next_finalized_payload_apply = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_panicking_network_pump_task(mut self) -> Self {
        self.panic_network_pump_task = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_automatic_rotation_start_failure(mut self) -> Self {
        self.fail_automatic_rotation_start = true;
        self
    }

    fn automatic_rotation_enabled(&self) -> bool {
        self.automatic_rotation_limit != Some(0)
    }

    pub(crate) fn after_automatic_rotation(&self) -> Self {
        let mut next = self.clone();
        next.replay_target_next_unit_index_by_member.clear();
        #[cfg(test)]
        {
            next.fail_before_task_spawn = next.fail_automatic_rotation_start;
            next.fail_automatic_rotation_start = false;
            next.publication_pause = None;
            next.root_task_activation_observer = None;
        }
        if let Some(limit) = next.automatic_rotation_limit {
            let remaining = limit.saturating_sub(1);
            if remaining == 0 {
                next.max_round = CDK_ALEPH_BFT_MAX_ROUND;
                next.automatic_rotation_limit = None;
            } else {
                next.automatic_rotation_limit = Some(remaining);
            }
        }
        next
    }

    fn without_sync_warmup_round_cap(mut self) -> Self {
        self.max_round = CDK_ALEPH_BFT_MAX_ROUND;
        self.automatic_rotation_limit = None;
        self
    }
}

impl Default for FederationAlephBftLiveSessionOptions {
    fn default() -> Self {
        Self {
            pump_config: CdkAlephBftNetworkPumpConfig::default(),
            max_round: CDK_ALEPH_BFT_MAX_ROUND,
            time_to_reach_max_round: Duration::from_secs(0),
            delay_config: None,
            automatic_rotation_limit: None,
            replay_target_next_unit_index_by_member: BTreeMap::new(),
            #[cfg(test)]
            panic_session_task: false,
            #[cfg(test)]
            fail_before_task_spawn: false,
            #[cfg(test)]
            fail_next_finalized_payload_apply: false,
            #[cfg(test)]
            panic_network_pump_task: false,
            #[cfg(test)]
            fail_automatic_rotation_start: false,
            #[cfg(test)]
            publication_pause: None,
            #[cfg(test)]
            root_task_activation_observer: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlephBftSessionCompletion {
    Completed,
    Panicked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AlephBftEssentialTaskEvent {
    Root(AlephBftSessionCompletion),
    FinalizedPayloadApplyFailed(String),
    FinalizedPayloadApplyPanicked,
    FinalizedPayloadApplyStopped,
    NetworkPumpPanicked,
    NetworkPumpStopped,
}

impl AlephBftEssentialTaskEvent {
    fn is_root_coupled_stop(&self) -> bool {
        matches!(
            self,
            Self::FinalizedPayloadApplyStopped | Self::NetworkPumpStopped
        )
    }
}

/// Router-startup decision for the live upstream AlephBFT session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationAlephBftLiveSessionStartupDecision {
    /// The mint is not configured as a federation member.
    NoFederationConfig,
    /// Federation state is missing its consensus runtime.
    MissingConsensusRuntime,
    /// A live session inbound queue is already installed.
    AlreadyStarted,
    /// AlephBFT is configured, but no remote peer can participate.
    NoRemotePeers {
        /// Local federation member.
        local_member: MemberId,
    },
    /// Spawn the live session for the configured remote peers.
    Spawn {
        /// Local federation member.
        local_member: MemberId,
        /// Number of remote peers used by the network pump.
        peer_count: usize,
        /// Current AlephBFT session.
        session_id: u64,
    },
}

/// Decide whether router startup should spawn the live upstream AlephBFT session.
pub(crate) async fn federation_aleph_bft_live_session_startup_decision(
    state: &MintState,
) -> Result<FederationAlephBftLiveSessionStartupDecision, cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(FederationAlephBftLiveSessionStartupDecision::NoFederationConfig);
    };
    if state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .is_some()
        || state
            .federation
            .aleph_bft_network_inbound
            .read()
            .await
            .is_some()
    {
        return Ok(FederationAlephBftLiveSessionStartupDecision::AlreadyStarted);
    }

    let Some(consensus) = &state.federation.consensus else {
        return Ok(FederationAlephBftLiveSessionStartupDecision::MissingConsensusRuntime);
    };
    let session_id = consensus.lock().await.aleph_bft_status()?.session;
    let local_member = config.member_id();
    let peer_count = config
        .public_config()
        .members
        .values()
        .filter(|member| member.member_id != local_member)
        .count();
    if peer_count == 0 {
        return Ok(FederationAlephBftLiveSessionStartupDecision::NoRemotePeers { local_member });
    }

    Ok(FederationAlephBftLiveSessionStartupDecision::Spawn {
        local_member,
        peer_count,
        session_id,
    })
}

/// Live upstream AlephBFT session tasks owned by router state.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct AlephBftSessionSupervisor {
    session_id: u64,
    generation: u64,
    state: Mutex<AlephBftSessionSupervisorState>,
    exit: Mutex<Option<oneshot::Sender<()>>>,
    activity_hint: CdkAlephBftActivityHint,
    replay_readiness: Arc<CdkAlephBftReplayReadiness>,
    spawn_handle: CdkAlephBftTokioSpawnHandle,
    activity_sender: mpsc::UnboundedSender<FederationOperationId>,
    tasks: Mutex<Option<AlephBftSessionTasks>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlephBftSessionSupervisorState {
    Constructing,
    Running,
    Stopping,
    Stopped,
}

struct AlephBftSessionTasks {
    session: JoinHandle<()>,
    activity_broadcast_task: JoinHandle<()>,
    network_pump_task: JoinHandle<()>,
    apply_task: JoinHandle<()>,
    completion_watcher_task: JoinHandle<()>,
}

impl AlephBftSessionTasks {
    fn abort(&self, abort_completion_watcher: bool) {
        self.session.abort();
        self.activity_broadcast_task.abort();
        self.network_pump_task.abort();
        self.apply_task.abort();
        if abort_completion_watcher {
            self.completion_watcher_task.abort();
        }
    }

    async fn join(self, join_completion_watcher: bool) {
        let _ = self.session.await;
        let _ = self.activity_broadcast_task.await;
        let _ = self.network_pump_task.await;
        let _ = self.apply_task.await;
        if join_completion_watcher {
            let _ = self.completion_watcher_task.await;
        }
    }
}

impl std::fmt::Debug for AlephBftSessionSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlephBftSessionSupervisor")
            .finish_non_exhaustive()
    }
}

impl AlephBftSessionSupervisor {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn replay_ready(&self) -> bool {
        self.replay_readiness.is_ready()
    }

    pub(crate) async fn wait_until_replay_ready(&self) {
        self.replay_readiness.wait_until_ready().await;
    }

    fn activate(&self) -> Result<(), cdk::Error> {
        let mut state = self.state.lock().map_err(|_| {
            cdk::Error::Custom("AlephBFT session supervisor state lock poisoned".to_string())
        })?;
        if *state != AlephBftSessionSupervisorState::Constructing {
            return Err(cdk::Error::Custom(
                "AlephBFT session supervisor was not constructing during activation".to_string(),
            ));
        }
        *state = AlephBftSessionSupervisorState::Running;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> AlephBftSessionSupervisorState {
        self.state
            .lock()
            .map(|state| *state)
            .unwrap_or(AlephBftSessionSupervisorState::Stopped)
    }

    #[cfg(test)]
    pub(crate) fn activity_is_active(&self) -> bool {
        self.activity_hint.is_active()
    }

    #[cfg(test)]
    pub(crate) fn clear_network_activity(&self) {
        self.activity_hint.clear_network_activity();
    }

    pub(crate) fn mark_activity(&self, operation_id: FederationOperationId, broadcast: bool) {
        self.activity_hint.mark_local_pending();
        if !broadcast {
            return;
        }
        if self.activity_sender.send(operation_id).is_err() {
            tracing::warn!(
                %operation_id,
                "AlephBFT activity broadcaster is not available"
            );
        }
    }

    pub(crate) fn mark_local_activity(&self) {
        self.activity_hint.mark_local_pending();
    }

    pub(crate) fn mark_peer_activity(&self, operation_id: FederationOperationId) {
        self.activity_hint
            .mark_network_activity([operation_id.to_bytes()]);
    }

    async fn shutdown(&self) {
        self.shutdown_inner(true).await;
    }

    async fn shutdown_for_rotation(&self) {
        self.shutdown_inner(false).await;
    }

    async fn shutdown_inner(&self, stop_completion_watcher: bool) {
        let should_stop = match self.state.lock() {
            Ok(mut state)
                if matches!(
                    *state,
                    AlephBftSessionSupervisorState::Constructing
                        | AlephBftSessionSupervisorState::Running
                ) =>
            {
                *state = AlephBftSessionSupervisorState::Stopping;
                true
            }
            _ => false,
        };
        if !should_stop {
            return;
        }
        if let Ok(mut exit) = self.exit.lock() {
            if let Some(exit) = exit.take() {
                let _ = exit.send(());
            }
        }
        self.spawn_handle.abort_all();
        let tasks = self.tasks.lock().ok().and_then(|mut tasks| tasks.take());
        if let Some(tasks) = tasks {
            tasks.abort(stop_completion_watcher);
            tasks.join(stop_completion_watcher).await;
        }
        self.spawn_handle.abort_all_and_wait().await;
        if let Ok(mut state) = self.state.lock() {
            *state = AlephBftSessionSupervisorState::Stopped;
        }
    }

    fn abort(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = AlephBftSessionSupervisorState::Stopped;
        }
        if let Ok(mut exit) = self.exit.lock() {
            if let Some(exit) = exit.take() {
                let _ = exit.send(());
            }
        }
        self.spawn_handle.abort_all();
        if let Ok(tasks) = self.tasks.lock() {
            if let Some(tasks) = tasks.as_ref() {
                tasks.abort(true);
            }
        }
    }
}

impl Drop for AlephBftSessionSupervisor {
    fn drop(&mut self) {
        self.abort();
    }
}

async fn await_aleph_bft_session_activation(mut activation: watch::Receiver<bool>) -> bool {
    loop {
        if *activation.borrow_and_update() {
            return true;
        }
        if activation.changed().await.is_err() {
            return false;
        }
    }
}

/// Start the live upstream AlephBFT session with its quorum-supported replay target.
pub(crate) async fn start_federation_aleph_bft_live_session_with_replay_target_and_transport<T>(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    replay_target_next_unit_index_by_member: BTreeMap<MemberId, u64>,
    transport: T,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    start_federation_aleph_bft_live_session_with_options_and_transport(
        state,
        sync_config,
        FederationAlephBftLiveSessionOptions::from_sync_config(
            CdkAlephBftNetworkPumpConfig::default(),
            sync_config,
        )
        .with_replay_target_next_unit_index_by_member(replay_target_next_unit_index_by_member),
        transport,
    )
    .await
}

/// Start the live upstream AlephBFT session with explicit testable work bounds.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn start_federation_aleph_bft_live_session_with_config(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error> {
    start_federation_aleph_bft_live_session_with_options_and_transport(
        state,
        sync_config,
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, sync_config),
        Async::default(),
    )
    .await
}

/// Start the live upstream AlephBFT session with an injected private REST transport.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn start_federation_aleph_bft_live_session_with_transport<T>(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
    transport: T,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    start_federation_aleph_bft_live_session_with_options_and_transport(
        state,
        sync_config,
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, sync_config),
        transport,
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn start_federation_aleph_bft_live_session_with_options_and_transport<T>(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    start_federation_aleph_bft_live_session_with_options_and_transport_inner(
        state,
        sync_config,
        options,
        transport,
        true,
    )
    .await
}

async fn start_federation_aleph_bft_live_session_with_options_and_transport_inner<T>(
    state: &MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
    restore_recovered_startup_round_cap: bool,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!("Invalid AlephBFT live session sync config: {err}"))
    })?;
    options.pump_config.validate()?;

    let decision = federation_aleph_bft_live_session_startup_decision(state).await?;
    let FederationAlephBftLiveSessionStartupDecision::Spawn { session_id, .. } = decision else {
        return Ok(None);
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id,
        ..options.pump_config
    };
    let halt_epoch = state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst);

    let config =
        state.mint.federation_config().cloned().ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session missing config".to_string())
        })?;
    let public_config = config.public_config().clone();
    let private_config = config.private_config().clone();
    let activity_peers = public_config
        .members
        .values()
        .filter(|member| member.member_id != private_config.member_id)
        .cloned()
        .collect::<Vec<_>>();
    let keychain = CdkAlephBftKeychain::try_new(&public_config, private_config.clone())?;
    let consensus = state
        .federation
        .consensus
        .as_ref()
        .cloned()
        .ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session missing consensus runtime".to_string())
        })?;
    let db = state.mint.localstore();
    let mut tx = db.begin_transaction().await.map_err(|err| {
        cdk::Error::Custom(format!(
            "AlephBFT active session transaction begin failed: {err}"
        ))
    })?;
    tx.set_federation_aleph_bft_active_session(session_id)
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!("AlephBFT active session persistence failed: {err}"))
        })?;
    tx.commit().await.map_err(|err| {
        cdk::Error::Custom(format!(
            "AlephBFT active session transaction commit failed: {err}"
        ))
    })?;
    let existing_session_backup_chunks = db
        .get_federation_aleph_bft_backup_chunks_from(session_id, 0, 1)
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!(
                "AlephBFT network retry backup lookup failed: {err}"
            ))
        })?;
    let has_existing_session_backup = !existing_session_backup_chunks.is_empty();
    let options = if restore_recovered_startup_round_cap {
        aleph_bft_live_session_options_for_startup(options, session_id, has_existing_session_backup)
    } else {
        options
    };
    let activity_hint = CdkAlephBftActivityHint::default();
    let session_io = CdkAlephBftSessionIo::from_db(
        Arc::clone(&consensus),
        Arc::clone(&db),
        session_id,
        options.replay_target_next_unit_index_by_member.clone(),
        activity_hint.clone(),
        keychain.clone(),
    )
    .await?;
    let replay_readiness = session_io.replay_readiness();
    let pending_activity_operation_ids = pending_aleph_bft_operation_ids(&db, session_id).await?;
    let (local_io, finalized_units) = session_io.into_parts();
    let (network, mut network_handle) = CdkAlephBftNetwork::<CdkAlephBftNetworkData>::new_pair();
    let inbound = Arc::new(CdkAlephBftNetworkInboundHandle::with_activity_hint(
        session_id,
        &network_handle,
        activity_hint.clone(),
    ));

    let aleph_config = aleph_bft_config(
        NodeCount(public_config.members.len()),
        keychain.index(),
        session_id,
        &options,
        activity_hint.clone(),
    )?;
    let rotation_transport = transport.clone();
    let client = FederationRestPeerClient::new_with_config(
        transport,
        &public_config,
        private_config,
        sync_config,
    )
    .map_err(|err| {
        cdk::Error::Custom(format!(
            "Could not create AlephBFT live session peer client: {err}"
        ))
    })?;
    #[cfg(test)]
    if options.fail_before_task_spawn {
        return Err(cdk::Error::Custom(
            "injected AlephBFT pre-spawn startup failure".to_string(),
        ));
    }
    let (exit_sender, exit_receiver) = oneshot::channel();
    let terminator = Terminator::create_root(exit_receiver, "cdk-aleph-bft-session");
    let spawn_handle = CdkAlephBftTokioSpawnHandle::default();
    let generation = next_aleph_bft_session_generation()?;
    let (essential_event_sender, essential_event_receiver) = mpsc::unbounded_channel();
    let (published_sender, published_receiver) = oneshot::channel();
    let (activation_sender, activation_receiver) = watch::channel(false);

    let session_keychain = keychain.clone();
    let session_spawn_handle = spawn_handle.clone();
    let root_event_sender = essential_event_sender.clone();
    let session_activation = activation_receiver.clone();
    #[cfg(test)]
    let panic_session_task = options.panic_session_task;
    #[cfg(test)]
    let root_task_activation_observer = options.root_task_activation_observer.clone();
    let session_task = tokio::spawn(async move {
        if !await_aleph_bft_session_activation(session_activation).await {
            return;
        }
        #[cfg(test)]
        if let Some(observer) = root_task_activation_observer {
            observer.store(true, Ordering::SeqCst);
        }
        let session = async move {
            #[cfg(test)]
            if panic_session_task {
                panic!("injected AlephBFT root session task panic");
            }
            run_session(
                aleph_config,
                local_io,
                network,
                session_keychain,
                session_spawn_handle,
                terminator,
            )
            .await;
        };
        let completion = match AssertUnwindSafe(session).catch_unwind().await {
            Ok(()) => AlephBftSessionCompletion::Completed,
            Err(_) => AlephBftSessionCompletion::Panicked,
        };
        let _ = root_event_sender.send(AlephBftEssentialTaskEvent::Root(completion));
    });
    let (activity_sender, activity_receiver) = mpsc::unbounded_channel();
    let activity_activation = activation_receiver.clone();
    let activity_client = client.clone();
    let activity_db = Arc::clone(&db);
    let activity_broadcast_hint = activity_hint.clone();
    let activity_broadcast_task = tokio::spawn(async move {
        if !await_aleph_bft_session_activation(activity_activation).await {
            return;
        }
        run_aleph_bft_activity_broadcast_loop(
            activity_client,
            activity_peers,
            session_id,
            activity_db,
            activity_broadcast_hint,
            pending_activity_operation_ids,
            activity_receiver,
        )
        .await;
    });
    let pump_keychain = keychain.clone();
    let network_event_sender = essential_event_sender.clone();
    let network_activation = activation_receiver.clone();
    #[cfg(test)]
    let panic_network_pump_task = options.panic_network_pump_task;
    let network_pump_task = tokio::spawn(async move {
        if !await_aleph_bft_session_activation(network_activation).await {
            return;
        }
        let pump = async move {
            #[cfg(test)]
            if panic_network_pump_task {
                panic!("injected AlephBFT network pump task panic");
            }
            let mut interval = federation_peer_sync_interval(sync_config);
            let mut retry_queue = CdkAlephBftNetworkRetryQueue::default();
            loop {
                let first_outbound = tokio::select! {
                    outbound = network_handle.next_outgoing() => match outbound {
                        Some(outbound) => Some(outbound),
                        None => {
                            tracing::warn!("AlephBFT network outbound bridge is closed");
                            break;
                        }
                    },
                    _ = interval.tick() => None,
                };
                match run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound(
                    &mut network_handle,
                    &pump_keychain,
                    &client,
                    pump_config,
                    &mut retry_queue,
                    first_outbound,
                )
                .await
                {
                    Ok(outcome) => {
                        if outcome.messages_drained > 0 || outcome.retry_messages_drained > 0 {
                            let remote_failed_members = outcome.remote_failed_members.len();
                            let remote_requeued_members = outcome.remote_requeued_members.len();
                            let remote_skipped_members = outcome.remote_skipped_members.len();
                            if remote_failed_members > 0
                                || remote_requeued_members > 0
                                || remote_skipped_members > 0
                            {
                                tracing::debug!(
                                    messages_drained = outcome.messages_drained,
                                    retry_messages_drained = outcome.retry_messages_drained,
                                    encoded_bytes = outcome.encoded_bytes,
                                    remote_accepted_members = outcome.remote_accepted_members.len(),
                                    remote_failed_members,
                                    remote_requeued_members,
                                    remote_skipped_members,
                                    retry_queue_len = outcome.retry_queue_len,
                                    retry_deliveries_dropped = outcome.retry_deliveries_dropped,
                                    reached_idle = outcome.reached_idle,
                                    "AlephBFT network pump tick completed with incomplete delivery"
                                );
                            } else {
                                tracing::trace!(
                                    messages_drained = outcome.messages_drained,
                                    retry_messages_drained = outcome.retry_messages_drained,
                                    encoded_bytes = outcome.encoded_bytes,
                                    remote_accepted_members = outcome.remote_accepted_members.len(),
                                    retry_queue_len = outcome.retry_queue_len,
                                    retry_deliveries_dropped = outcome.retry_deliveries_dropped,
                                    reached_idle = outcome.reached_idle,
                                    "AlephBFT network pump tick completed"
                                );
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!("AlephBFT network pump tick failed: {}", err);
                    }
                }
            }
        };
        match AssertUnwindSafe(pump).catch_unwind().await {
            Ok(()) => {
                let _ = network_event_sender.send(AlephBftEssentialTaskEvent::NetworkPumpStopped);
            }
            Err(_) => {
                let _ = network_event_sender.send(AlephBftEssentialTaskEvent::NetworkPumpPanicked);
            }
        }
    });

    let apply_state = state.clone();
    let apply_event_sender = essential_event_sender;
    let apply_activation = activation_receiver;
    #[cfg(test)]
    let fail_next_finalized_payload_apply = options.fail_next_finalized_payload_apply;
    #[cfg(not(test))]
    let fail_next_finalized_payload_apply = false;
    let apply_task = tokio::spawn(async move {
        if !await_aleph_bft_session_activation(apply_activation).await {
            return;
        }
        let apply_loop = run_federation_aleph_bft_finalized_unit_apply_loop(
            apply_state,
            finalized_units,
            fail_next_finalized_payload_apply,
        );
        match AssertUnwindSafe(apply_loop).catch_unwind().await {
            Ok(Ok(())) => {
                let _ = apply_event_sender
                    .send(AlephBftEssentialTaskEvent::FinalizedPayloadApplyStopped);
            }
            Ok(Err(err)) => {
                tracing::error!(
                    "AlephBFT finalized-payload apply task stopped after a non-retryable error: {}",
                    err
                );
                let _ = apply_event_sender.send(
                    AlephBftEssentialTaskEvent::FinalizedPayloadApplyFailed(err.to_string()),
                );
            }
            Err(_) => {
                let _ = apply_event_sender
                    .send(AlephBftEssentialTaskEvent::FinalizedPayloadApplyPanicked);
            }
        }
    });
    #[cfg(test)]
    let publication_pause = options.publication_pause.clone();
    let completion_watcher_task = spawn_federation_aleph_bft_session_completion_watcher(
        state.clone(),
        sync_config,
        options,
        rotation_transport,
        session_id,
        generation,
        published_receiver,
        essential_event_receiver,
    );
    let handle = Arc::new(AlephBftSessionSupervisor {
        session_id,
        generation,
        state: Mutex::new(AlephBftSessionSupervisorState::Constructing),
        exit: Mutex::new(Some(exit_sender)),
        activity_hint,
        replay_readiness,
        spawn_handle,
        activity_sender,
        tasks: Mutex::new(Some(AlephBftSessionTasks {
            session: session_task,
            activity_broadcast_task,
            network_pump_task,
            apply_task,
            completion_watcher_task,
        })),
    });
    #[cfg(test)]
    if let Some((reached, release)) = publication_pause {
        reached.notify_one();
        release.notified().await;
    }
    let mut live_session = state.federation.aleph_bft_live_session.write().await;
    if state.federation.aleph_bft_halt_epoch.load(Ordering::SeqCst) != halt_epoch {
        drop(live_session);
        handle.shutdown().await;
        return Err(cdk::Error::Custom(
            "AlephBFT live session became halted before publication".to_string(),
        ));
    }
    if live_session.is_some() {
        drop(live_session);
        handle.shutdown().await;
        return Err(cdk::Error::Custom(
            "AlephBFT live session was concurrently published".to_string(),
        ));
    }
    if let Err(err) = handle.activate() {
        drop(live_session);
        handle.shutdown().await;
        return Err(err);
    }
    let mut network_inbound = state.federation.aleph_bft_network_inbound.write().await;
    *network_inbound = Some(inbound);
    *live_session = Some(Arc::clone(&handle));
    drop(network_inbound);
    drop(live_session);
    spawn_federation_aleph_bft_replay_readiness_watcher(state.clone(), Arc::clone(&handle));
    let _ = published_sender.send(());
    activation_sender.send_replace(true);

    Ok(Some(handle))
}

fn spawn_federation_aleph_bft_replay_readiness_watcher(
    state: MintState,
    handle: Arc<AlephBftSessionSupervisor>,
) {
    tokio::spawn(async move {
        handle.wait_until_replay_ready().await;
        let live_session = state.federation.aleph_bft_live_session.read().await;
        if live_session
            .as_ref()
            .is_none_or(|current| current.generation() != handle.generation())
            || matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::Halted { .. })
            )
        {
            return;
        }
        state.federation.mark_healthy_after_replay().await;
        drop(live_session);
    });
}

async fn pending_aleph_bft_operation_ids(
    db: &DynMintDatabase,
    session_id: u64,
) -> Result<BTreeSet<FederationOperationId>, cdk::Error> {
    Ok(db
        .get_federation_aleph_bft_pending_operations(session_id)
        .await?
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect())
}

async fn run_aleph_bft_activity_broadcast_loop<T>(
    client: FederationRestPeerClient<T>,
    peers: Vec<MemberConfigPublic>,
    session_id: u64,
    db: DynMintDatabase,
    activity_hint: CdkAlephBftActivityHint,
    mut pending_operation_ids: BTreeSet<FederationOperationId>,
    mut activity_receiver: mpsc::UnboundedReceiver<FederationOperationId>,
) where
    T: Transport + 'static,
{
    if !pending_operation_ids.is_empty() {
        refresh_aleph_bft_operation_activity(
            &client,
            &peers,
            session_id,
            &activity_hint,
            &pending_operation_ids,
            false,
        )
        .await;
    }

    loop {
        if pending_operation_ids.is_empty() {
            let Some(operation_id) = activity_receiver.recv().await else {
                return;
            };
            pending_operation_ids.insert(operation_id);
            refresh_aleph_bft_operation_activity(
                &client,
                &peers,
                session_id,
                &activity_hint,
                &BTreeSet::from([operation_id]),
                true,
            )
            .await;
            continue;
        }

        tokio::select! {
            operation_id = activity_receiver.recv() => {
                let Some(operation_id) = operation_id else {
                    return;
                };
                pending_operation_ids.insert(operation_id);
                refresh_aleph_bft_operation_activity(
                    &client,
                    &peers,
                    session_id,
                    &activity_hint,
                    &BTreeSet::from([operation_id]),
                    true,
                )
                .await;
            }
            () = tokio::time::sleep(aleph_bft_activity_refresh_interval()) => {
                match pending_aleph_bft_operation_ids(&db, session_id).await {
                    Ok(durable_operation_ids) => {
                        pending_operation_ids = durable_operation_ids;
                    }
                    Err(err) => {
                        tracing::warn!(
                            session_id,
                            "AlephBFT activity refresh could not load pending operations: {}",
                            err
                        );
                    }
                }
                if !pending_operation_ids.is_empty() {
                    refresh_aleph_bft_operation_activity(
                        &client,
                        &peers,
                        session_id,
                        &activity_hint,
                        &pending_operation_ids,
                        false,
                    )
                    .await;
                }
            }
        }
    }
}

fn aleph_bft_activity_refresh_interval() -> Duration {
    let interval = CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE / 2;
    if interval.is_zero() {
        Duration::from_millis(1)
    } else {
        interval
    }
}

async fn refresh_aleph_bft_operation_activity<T>(
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    session_id: u64,
    activity_hint: &CdkAlephBftActivityHint,
    operation_ids: &BTreeSet<FederationOperationId>,
    warn_if_unreachable: bool,
) where
    T: Transport,
{
    // One operation-bound lease is enough to keep the shared session active.
    let Some(operation_id) = operation_ids.first().copied() else {
        return;
    };
    activity_hint.mark_local_pending();
    broadcast_aleph_bft_operation_activity(
        client,
        peers,
        session_id,
        operation_id,
        warn_if_unreachable,
    )
    .await;
}

async fn broadcast_aleph_bft_operation_activity<T>(
    client: &FederationRestPeerClient<T>,
    peers: &[MemberConfigPublic],
    session_id: u64,
    operation_id: FederationOperationId,
    warn_if_unreachable: bool,
) where
    T: Transport,
{
    let request = FederationAlephBftActivityRequest::new(session_id, operation_id);
    let outcomes = join_all(
        peers
            .iter()
            .map(|peer| client.submit_aleph_bft_activity(peer, &request)),
    )
    .await;
    let mut accepted = 0usize;
    let mut failed = 0usize;
    for outcome in outcomes {
        match outcome {
            Ok(_) => accepted = accepted.saturating_add(1),
            Err(_) => failed = failed.saturating_add(1),
        }
    }
    if accepted == 0 {
        if warn_if_unreachable {
            tracing::warn!(
                %operation_id,
                session_id,
                failed_members = failed,
                "AlephBFT activity wake reached no remote member"
            );
        } else {
            tracing::debug!(
                %operation_id,
                session_id,
                failed_members = failed,
                "AlephBFT pending-work activity refresh reached no remote member"
            );
        }
    } else if failed > 0 {
        tracing::debug!(
            %operation_id,
            session_id,
            accepted_members = accepted,
            failed_members = failed,
            "AlephBFT activity wake reached only part of the federation"
        );
    }
}

fn aleph_bft_config(
    node_count: NodeCount,
    node_index: aleph_bft::NodeIndex,
    session_id: u64,
    options: &FederationAlephBftLiveSessionOptions,
    activity_hint: CdkAlephBftActivityHint,
) -> Result<AlephBftConfig, cdk::Error> {
    let config = match options.delay_config.clone() {
        Some(delay_config) => create_config(
            node_count,
            node_index,
            session_id,
            options.max_round,
            delay_config,
            options.time_to_reach_max_round,
        ),
        None => create_config(
            node_count,
            node_index,
            session_id,
            options.max_round,
            cdk_aleph_bft_delay_config_with_activity(activity_hint),
            options.time_to_reach_max_round,
        ),
    };
    config.map_err(|_| cdk::Error::Custom("Invalid AlephBFT live session config".to_string()))
}

fn aleph_bft_live_session_options_for_startup(
    options: FederationAlephBftLiveSessionOptions,
    session_id: u64,
    has_existing_session_backup: bool,
) -> FederationAlephBftLiveSessionOptions {
    if options.automatic_rotation_limit.is_some() && (session_id > 0 || has_existing_session_backup)
    {
        return options.without_sync_warmup_round_cap();
    }

    options
}

#[cfg(test)]
fn cdk_aleph_bft_delay_config() -> DelayConfig {
    cdk_aleph_bft_delay_config_with_activity(CdkAlephBftActivityHint::default())
}

fn cdk_aleph_bft_delay_config_with_activity(activity_hint: CdkAlephBftActivityHint) -> DelayConfig {
    let mut delay_config = default_delay_config();
    let default_unit_creation_delay = Arc::clone(&delay_config.unit_creation_delay);
    delay_config.unit_creation_delay = Arc::new(move |round| {
        if activity_hint.is_active() {
            // Keep AlephBFT's round-aware cadence so empty helper units cannot
            // outrun dissemination while real work awaits finalization.
            default_unit_creation_delay(round)
        } else if round > CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND {
            CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        } else {
            default_unit_creation_delay(round)
        }
    });
    delay_config
}

fn spawn_federation_aleph_bft_session_completion_watcher<T>(
    state: MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
    completed_session: u64,
    generation: u64,
    published_receiver: oneshot::Receiver<()>,
    mut essential_event_receiver: mpsc::UnboundedReceiver<AlephBftEssentialTaskEvent>,
) -> JoinHandle<()>
where
    T: Transport + 'static,
{
    tokio::spawn(async move {
        if published_receiver.await.is_err() {
            return;
        }
        let Some(mut event) = essential_event_receiver.recv().await else {
            tracing::debug!(
                completed_session,
                generation,
                "AlephBFT live session stopped before an essential task outcome"
            );
            return;
        };
        if event.is_root_coupled_stop() {
            if let Ok(Some(root_or_failure)) =
                tokio::time::timeout(CDK_ALEPH_BFT_ESSENTIAL_TASK_ROOT_EXIT_GRACE, async {
                    while let Some(next) = essential_event_receiver.recv().await {
                        if !next.is_root_coupled_stop() {
                            return Some(next);
                        }
                    }
                    None
                })
                .await
            {
                event = root_or_failure;
            }
        }
        match event {
            AlephBftEssentialTaskEvent::Root(AlephBftSessionCompletion::Completed) => {
                if !options.automatic_rotation_enabled() {
                    halt_federation_aleph_bft_generation(
                        &state,
                        completed_session,
                        FederationAlephBftHaltTarget::Generation(generation),
                        "completed with automatic rotation disabled",
                        true,
                    )
                    .await;
                    return;
                }
                let mut state = state;
                let next_options = options.after_automatic_rotation();
                if let Err(err) = Box::pin(
                    rotate_federation_aleph_bft_completed_live_session_with_options_and_transport(
                        &mut state,
                        sync_config,
                        next_options,
                        transport,
                        completed_session,
                    ),
                )
                .await
                {
                    tracing::warn!(
                        completed_session,
                        "AlephBFT automatic live-session rotation failed: {}",
                        err
                    );
                    halt_federation_aleph_bft_generation(
                        &state,
                        completed_session,
                        FederationAlephBftHaltTarget::GenerationOrNone(generation),
                        "automatic rotation failed",
                        true,
                    )
                    .await;
                }
            }
            AlephBftEssentialTaskEvent::Root(AlephBftSessionCompletion::Panicked) => {
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    "stopped after a root task panic",
                    true,
                )
                .await;
            }
            AlephBftEssentialTaskEvent::FinalizedPayloadApplyFailed(error) => {
                let reason = format!(
                    "stopped after a non-retryable finalized-payload apply failure: {error}"
                );
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    &reason,
                    true,
                )
                .await;
            }
            AlephBftEssentialTaskEvent::FinalizedPayloadApplyPanicked => {
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    "stopped after the finalized-payload apply task panicked",
                    true,
                )
                .await;
            }
            AlephBftEssentialTaskEvent::FinalizedPayloadApplyStopped => {
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    "stopped after the finalized-payload apply task exited unexpectedly",
                    true,
                )
                .await;
            }
            AlephBftEssentialTaskEvent::NetworkPumpPanicked => {
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    "stopped after the network pump task panicked",
                    true,
                )
                .await;
            }
            AlephBftEssentialTaskEvent::NetworkPumpStopped => {
                halt_federation_aleph_bft_generation(
                    &state,
                    completed_session,
                    FederationAlephBftHaltTarget::Generation(generation),
                    "stopped after the network pump task exited unexpectedly",
                    true,
                )
                .await;
            }
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FederationAlephBftHaltTarget {
    Current,
    Generation(u64),
    GenerationOrNone(u64),
}

async fn halt_federation_aleph_bft_generation(
    state: &MintState,
    session_id: u64,
    target: FederationAlephBftHaltTarget,
    reason: &str,
    called_from_completion_watcher: bool,
) {
    let mut live_session = state.federation.aleph_bft_live_session.write().await;
    let target_matches = match (target, live_session.as_ref()) {
        (FederationAlephBftHaltTarget::Current, _) => true,
        (FederationAlephBftHaltTarget::Generation(expected), Some(supervisor)) => {
            supervisor.generation() == expected
        }
        (FederationAlephBftHaltTarget::Generation(_), None) => false,
        (FederationAlephBftHaltTarget::GenerationOrNone(expected), Some(supervisor)) => {
            supervisor.generation() == expected
        }
        (FederationAlephBftHaltTarget::GenerationOrNone(_), None) => true,
    };
    if !target_matches {
        tracing::debug!(
            session_id,
            ?target,
            "Ignoring stale AlephBFT halt from an inactive live-session generation"
        );
        return;
    }

    let supervisor = live_session.take();
    let mut network_inbound = state.federation.aleph_bft_network_inbound.write().await;
    *network_inbound = None;
    state
        .federation
        .aleph_bft_halt_epoch
        .fetch_add(1, Ordering::SeqCst);
    tracing::error!(session_id, reason, "AlephBFT live session halted");
    state
        .mark_federation_halted(format!("AlephBFT live session {session_id} {reason}"))
        .await;
    drop(network_inbound);
    drop(live_session);
    if let Some(supervisor) = supervisor {
        if called_from_completion_watcher {
            supervisor.shutdown_for_rotation().await;
        } else {
            supervisor.shutdown().await;
        }
    }
}

pub(crate) async fn halt_current_federation_aleph_bft_session(state: &MintState, reason: &str) {
    let session_id = state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .as_ref()
        .map_or(0, |supervisor| supervisor.session_id);
    halt_federation_aleph_bft_generation(
        state,
        session_id,
        FederationAlephBftHaltTarget::Current,
        reason,
        false,
    )
    .await;
}

pub(crate) async fn stop_current_federation_aleph_bft_session_for_catch_up(state: &MintState) {
    let mut live_session = state.federation.aleph_bft_live_session.write().await;
    let supervisor = live_session.take();
    let mut network_inbound = state.federation.aleph_bft_network_inbound.write().await;
    *network_inbound = None;
    state
        .federation
        .aleph_bft_halt_epoch
        .fetch_add(1, Ordering::SeqCst);
    drop(network_inbound);
    drop(live_session);
    if let Some(supervisor) = supervisor {
        tracing::info!(
            session_id = supervisor.session_id,
            "Stopping stale AlephBFT live session before checkpoint catch-up"
        );
        supervisor.shutdown().await;
    }
}

#[cfg(test)]
pub(crate) async fn halt_federation_aleph_bft_generation_for_test(
    state: &MintState,
    session_id: u64,
    generation: u64,
) {
    halt_federation_aleph_bft_generation(
        state,
        session_id,
        FederationAlephBftHaltTarget::Generation(generation),
        "injected stale generation failure",
        false,
    )
    .await;
}

/// Rotate a live upstream AlephBFT task into the next empty session.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn rotate_federation_aleph_bft_live_session_to_next_empty_with_transport<T>(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
    transport: T,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    rotate_federation_aleph_bft_live_session_to_next_empty_with_options_and_transport(
        state,
        sync_config,
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, sync_config),
        transport,
    )
    .await
}

async fn rotate_federation_aleph_bft_live_session_to_next_empty_with_options_and_transport<T>(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!(
            "Invalid AlephBFT live session rotation sync config: {err}"
        ))
    })?;
    options.pump_config.validate()?;

    if state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .is_none()
    {
        return Ok(None);
    }

    let Some(consensus) = state.federation.consensus.as_ref().cloned() else {
        return Err(cdk::federation::FederationError::RuntimeUnavailable {
            component: "consensus",
        }
        .into());
    };

    let next_session = {
        let consensus = consensus.lock().await;
        let status = consensus.aleph_bft_status().map_err(cdk::Error::from)?;
        let pending = consensus.pending_mempool_len();
        if pending != 0 {
            return Err(
                cdk::federation::FederationError::AlephBftSessionRotationBlocked {
                    session: status.session,
                    pending_items: pending,
                }
                .into(),
            );
        }
        let next_session = status.session.checked_add(1).ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session rotation overflowed session id".to_string())
        })?;
        next_session
    };

    let previous_handle = state.federation.aleph_bft_live_session.write().await.take();
    if let Some(handle) = previous_handle {
        handle.shutdown().await;
    }
    *state.federation.aleph_bft_network_inbound.write().await = None;

    {
        let mut consensus = consensus.lock().await;
        let status = consensus.aleph_bft_status().map_err(cdk::Error::from)?;
        let pending = consensus.pending_mempool_len();
        if pending != 0 {
            return Err(
                cdk::federation::FederationError::AlephBftSessionRotationBlocked {
                    session: status.session,
                    pending_items: pending,
                }
                .into(),
            );
        }
        consensus
            .begin_aleph_bft_session(next_session)
            .map_err(cdk::Error::from)?;
    }

    match start_federation_aleph_bft_live_session_with_options_and_transport_inner(
        state,
        sync_config,
        options,
        transport,
        false,
    )
    .await?
    {
        Some(handle) => Ok(Some(handle)),
        None => Err(cdk::Error::Custom(
            "AlephBFT live session rotation could not restart the live session".to_string(),
        )),
    }
}

async fn rotate_federation_aleph_bft_completed_live_session_with_options_and_transport<T>(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
    completed_session: u64,
) -> Result<Option<Arc<AlephBftSessionSupervisor>>, cdk::Error>
where
    T: Transport + 'static,
{
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!(
            "Invalid AlephBFT completed-session rotation sync config: {err}"
        ))
    })?;
    options.pump_config.validate()?;

    if state
        .federation
        .aleph_bft_live_session
        .read()
        .await
        .is_none()
    {
        return Ok(None);
    }

    let Some(consensus) = state.federation.consensus.as_ref().cloned() else {
        return Err(cdk::Error::Custom(
            "AlephBFT completed-session rotation requires federation consensus runtime".to_string(),
        ));
    };

    let next_session = {
        let mut consensus = consensus.lock().await;
        let status = consensus.aleph_bft_status().map_err(cdk::Error::from)?;
        if status.session != completed_session {
            return Ok(None);
        }
        let next_session = status.session.checked_add(1).ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session rotation overflowed session id".to_string())
        })?;
        let requeued_items = consensus
            .begin_aleph_bft_session_requeue_pending_finalization(next_session)
            .map_err(cdk::Error::from)?;
        if requeued_items > 0 {
            tracing::warn!(
                completed_session,
                next_session,
                requeued_items,
                "AlephBFT completed-session rotation requeued unfinalized local items"
            );
        }
        next_session
    };

    let previous_handle = state.federation.aleph_bft_live_session.write().await.take();
    if let Some(handle) = previous_handle {
        handle.shutdown_for_rotation().await;
    }
    *state.federation.aleph_bft_network_inbound.write().await = None;

    tracing::info!(
        completed_session,
        next_session,
        "AlephBFT live session completed; rotating automatically"
    );

    match start_federation_aleph_bft_live_session_with_options_and_transport_inner(
        state,
        sync_config,
        options,
        transport,
        false,
    )
    .await?
    {
        Some(handle) => Ok(Some(handle)),
        None => Err(cdk::Error::Custom(
            "AlephBFT live session rotation could not restart the live session".to_string(),
        )),
    }
}

async fn run_federation_aleph_bft_finalized_unit_apply_loop(
    state: MintState,
    mut finalized_units: tokio::sync::mpsc::UnboundedReceiver<CdkAlephBftFinalizedUnit>,
    fail_next_finalized_payload_apply: bool,
) -> Result<(), cdk::Error> {
    while let Some(finalized_unit) = finalized_units.recv().await {
        let creator = finalized_unit.creator();
        let round = finalized_unit.round();
        let operation_ids = finalized_unit.operation_ids().to_vec();
        let payload = match finalized_unit.into_authenticated_payload() {
            Ok(payload) => payload,
            Err(err) => {
                tracing::warn!(
                    %creator,
                    round,
                    "Ignoring malformed finalized AlephBFT unit: {}",
                    err
                );
                continue;
            }
        };
        crate::federation::aleph_bft_latency::record(
            crate::federation::aleph_bft_latency::CdkAlephBftLatencyStage::ApplyLoopReceived,
            operation_ids,
        );
        if fail_next_finalized_payload_apply {
            return Err(cdk::Error::AmountOverflow);
        }
        match apply_federation_aleph_bft_finalized_unit_with_retry(&state, payload).await {
            Ok(()) => {}
            Err(cdk::Error::Federation(FederationError::DuplicateAlephBftUnit { .. })) => {
                tracing::warn!(
                    %creator,
                    round,
                    "Ignoring duplicate finalized AlephBFT unit"
                );
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

async fn apply_federation_aleph_bft_finalized_unit_with_retry(
    state: &MintState,
    payload: cdk::federation::FederationAlephBftUnitPayload,
) -> Result<(), cdk::Error> {
    let mut attempts = 0usize;
    let mut retry_delay = FEDERATION_APPLY_RETRY_INITIAL_DELAY;

    loop {
        match state
            .apply_federation_aleph_bft_finalized_unit(payload.clone())
            .await
        {
            Ok(_) => {
                if attempts > 0 {
                    tracing::info!(
                        retry_attempts = attempts,
                        "AlephBFT finalized-unit apply recovered after transient storage contention"
                    );
                }
                return Ok(());
            }
            Err(err) if is_retryable_federation_apply_error(&err) => {
                attempts = attempts.saturating_add(1);
                if attempts == 1 || attempts % 10 == 0 {
                    tracing::warn!(
                        retry_attempts = attempts,
                        retry_delay_ms = retry_delay.as_millis(),
                        "AlephBFT finalized-unit apply hit transient storage contention; retrying"
                    );
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = retry_delay
                    .saturating_mul(2)
                    .min(FEDERATION_APPLY_RETRY_MAX_DELAY);
            }
            Err(err) => {
                return Err(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cdk::federation::{
        FederationAlephBftPendingOperationRecord, FederationId, FederationOperation,
        FederationOperationEnvelope, FederationSwapOperation,
    };
    use cdk::nuts::SwapRequest;

    use super::*;

    struct TaskDropCounter {
        stopped: Arc<AtomicUsize>,
    }

    impl Drop for TaskDropCounter {
        fn drop(&mut self) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn pending_supervised_task(
        started: Arc<AtomicUsize>,
        stopped: Arc<AtomicUsize>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let _drop_counter = TaskDropCounter { stopped };
            started.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        })
    }

    #[tokio::test]
    async fn aleph_bft_session_supervisor_shutdown_joins_every_owned_task() {
        let started = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let task = || pending_supervised_task(Arc::clone(&started), Arc::clone(&stopped));
        let (exit_sender, _exit_receiver) = oneshot::channel();
        let (activity_sender, _activity_receiver) = mpsc::unbounded_channel();
        let supervisor = AlephBftSessionSupervisor {
            session_id: 0,
            generation: 1,
            state: Mutex::new(AlephBftSessionSupervisorState::Running),
            exit: Mutex::new(Some(exit_sender)),
            activity_hint: CdkAlephBftActivityHint::default(),
            replay_readiness: Arc::new(CdkAlephBftReplayReadiness::ready()),
            spawn_handle: CdkAlephBftTokioSpawnHandle::default(),
            activity_sender,
            tasks: Mutex::new(Some(AlephBftSessionTasks {
                session: task(),
                activity_broadcast_task: task(),
                network_pump_task: task(),
                apply_task: task(),
                completion_watcher_task: task(),
            })),
        };

        while started.load(Ordering::SeqCst) < 5 {
            tokio::task::yield_now().await;
        }
        supervisor.shutdown().await;

        assert_eq!(supervisor.state(), AlephBftSessionSupervisorState::Stopped);
        assert_eq!(stopped.load(Ordering::SeqCst), 5);
        assert!(supervisor
            .tasks
            .lock()
            .expect("supervisor tasks lock")
            .is_none());
    }

    #[tokio::test]
    async fn pending_activity_refresh_loads_only_current_session_operations() {
        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("pending activity database"),
        );
        assert!(pending_aleph_bft_operation_ids(&db, 7)
            .await
            .expect("empty pending activity lookup")
            .is_empty());

        let envelope = FederationOperationEnvelope::new(
            FederationId::from_bytes([7; 32]),
            FederationOperation::Swap(FederationSwapOperation {
                request: SwapRequest::new(Vec::new(), Vec::new()),
            }),
        );
        let operation_id = envelope.operation_id().expect("pending operation id");
        let pending = FederationAlephBftPendingOperationRecord {
            operation_id,
            session_id: 7,
            envelope,
        };
        let other_envelope = FederationOperationEnvelope::new(
            FederationId::from_bytes([8; 32]),
            FederationOperation::Swap(FederationSwapOperation {
                request: SwapRequest::new(Vec::new(), Vec::new()),
            }),
        );
        let other_session = FederationAlephBftPendingOperationRecord {
            operation_id: other_envelope
                .operation_id()
                .expect("other pending operation id"),
            session_id: 8,
            envelope: other_envelope,
        };
        let mut tx = db.begin_transaction().await.expect("pending operation tx");
        tx.add_federation_aleph_bft_pending_operation(&pending)
            .await
            .expect("persist pending operation");
        tx.add_federation_aleph_bft_pending_operation(&other_session)
            .await
            .expect("persist other-session operation");
        tx.commit().await.expect("commit pending operations");

        assert_eq!(
            pending_aleph_bft_operation_ids(&db, 7)
                .await
                .expect("pending activity lookup"),
            BTreeSet::from([operation_id])
        );
    }

    #[test]
    fn pending_activity_refresh_renews_before_network_activity_expires() {
        let refresh_interval = aleph_bft_activity_refresh_interval();

        assert!(!refresh_interval.is_zero());
        assert!(refresh_interval < CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE);
    }

    #[test]
    fn finalized_unit_apply_retry_predicate_accepts_transient_storage_contention() {
        let sqlite_locked = cdk::Error::Database(cdk::cdk_database::Error::Database(Box::new(
            io::Error::new(io::ErrorKind::Other, "database is locked"),
        )));
        let sqlite_busy = cdk::Error::Database(cdk::cdk_database::Error::Database(Box::new(
            io::Error::new(io::ErrorKind::Other, "DatabaseBusy"),
        )));

        assert!(is_retryable_federation_apply_error(
            &cdk::Error::ConcurrentUpdate
        ));
        assert!(is_retryable_federation_apply_error(&cdk::Error::Database(
            cdk::cdk_database::Error::Locked
        )));
        assert!(is_retryable_federation_apply_error(&sqlite_locked));
        assert!(is_retryable_federation_apply_error(&sqlite_busy));
    }

    #[test]
    fn finalized_unit_apply_retry_predicate_rejects_consensus_validation_errors() {
        assert!(!is_retryable_federation_apply_error(
            &cdk::Error::AmountOverflow
        ));
        assert!(!is_retryable_federation_apply_error(&cdk::Error::Database(
            cdk::cdk_database::Error::UnknownQuote
        )));
        assert!(!is_retryable_federation_apply_error(&cdk::Error::Database(
            cdk::cdk_database::Error::InvalidDbResponse
        )));
    }

    #[test]
    fn aleph_bft_live_session_options_restore_unlimited_rotation_after_sync_warmup() {
        let options = FederationAlephBftLiveSessionOptions::default()
            .with_max_round(8)
            .with_automatic_rotation_limit(2)
            .with_replay_target_next_unit_index_by_member(BTreeMap::from([(
                MemberId::new(1).expect("valid member ID"),
                7,
            )]));

        let first_rotation = options.after_automatic_rotation();
        assert_eq!(first_rotation.max_round(), 8);
        assert_eq!(first_rotation.automatic_rotation_limit(), Some(1));
        assert!(first_rotation
            .replay_target_next_unit_index_by_member
            .is_empty());

        let second_rotation = first_rotation.after_automatic_rotation();
        assert_eq!(second_rotation.max_round(), CDK_ALEPH_BFT_MAX_ROUND);
        assert_eq!(second_rotation.automatic_rotation_limit(), None);

        let third_rotation = second_rotation.after_automatic_rotation();
        assert_eq!(third_rotation.max_round(), CDK_ALEPH_BFT_MAX_ROUND);
        assert_eq!(third_rotation.automatic_rotation_limit(), None);
    }

    #[test]
    fn aleph_bft_live_session_startup_round_cap_is_fresh_session_only() {
        let options = FederationAlephBftLiveSessionOptions::default()
            .with_max_round(8)
            .with_automatic_rotation_limit(1);

        let fresh_options = aleph_bft_live_session_options_for_startup(options.clone(), 0, false);
        assert_eq!(fresh_options.max_round(), 8);
        assert_eq!(fresh_options.automatic_rotation_limit(), Some(1));

        let backup_recovered_options =
            aleph_bft_live_session_options_for_startup(options.clone(), 0, true);
        assert_eq!(
            backup_recovered_options.max_round(),
            CDK_ALEPH_BFT_MAX_ROUND
        );
        assert_eq!(backup_recovered_options.automatic_rotation_limit(), None);

        let later_session_options = aleph_bft_live_session_options_for_startup(options, 1, false);
        assert_eq!(later_session_options.max_round(), CDK_ALEPH_BFT_MAX_ROUND);
        assert_eq!(later_session_options.automatic_rotation_limit(), None);
    }

    #[test]
    fn aleph_bft_live_session_default_delay_slows_idle_tail_rounds() {
        let delay_config = cdk_aleph_bft_delay_config();
        let early_delay =
            (delay_config.unit_creation_delay)(CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND);
        let tail_delay =
            (delay_config.unit_creation_delay)(CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND + 1);

        assert_eq!(tail_delay, CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY);
        assert!(tail_delay > early_delay);
    }

    #[test]
    fn aleph_bft_live_session_activity_hint_keeps_tail_rounds_on_active_cadence() {
        let activity_hint = CdkAlephBftActivityHint::default();
        let delay_config = cdk_aleph_bft_delay_config_with_activity(activity_hint.clone());
        let default_delay_config = default_delay_config();
        let early_round = CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND;
        let idle_early_delay = (delay_config.unit_creation_delay)(early_round);
        let tail_round = CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND + 1;

        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        );

        activity_hint.mark_local_pending();
        assert_eq!(
            (delay_config.unit_creation_delay)(early_round),
            (default_delay_config.unit_creation_delay)(early_round)
        );
        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            (default_delay_config.unit_creation_delay)(tail_round)
        );
        assert_eq!(
            (delay_config.unit_creation_delay)(early_round),
            idle_early_delay
        );
        assert!(
            (delay_config.unit_creation_delay)(tail_round) < CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        );

        activity_hint.set_local_pending(false);
        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        );

        activity_hint.mark_network_activity([[1u8; 32]]);
        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            (default_delay_config.unit_creation_delay)(tail_round)
        );
    }
}
