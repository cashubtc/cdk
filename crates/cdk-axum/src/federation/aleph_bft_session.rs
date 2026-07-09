use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::{
    create_config, default_delay_config, run_session, Config as AlephBftConfig, DelayConfig, Index,
    NodeCount, Terminator,
};
use cdk::federation::{FederationAlephBftFinalityMode, FederationConsensusBackendKind, MemberId};
use cdk_http_client::{Async, Transport};
use futures::channel::oneshot;
use tokio::task::JoinHandle;

use crate::MintState;

use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue_and_first_outbound,
    CdkAlephBftNetworkInboundHandle, CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
};
use super::aleph_bft_runtime::{
    CdkAlephBftActivityHint, CdkAlephBftKeychain, CdkAlephBftNetwork, CdkAlephBftNetworkData,
    CdkAlephBftSessionIo, CdkAlephBftTokioSpawnHandle,
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

#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationAlephBftLiveSessionOptions {
    pump_config: CdkAlephBftNetworkPumpConfig,
    max_round: u16,
    time_to_reach_max_round: Duration,
    delay_config: Option<DelayConfig>,
    automatic_rotation_limit: Option<usize>,
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

    #[cfg(test)]
    pub(crate) fn automatic_rotation_limit(&self) -> Option<usize> {
        self.automatic_rotation_limit
    }

    fn automatic_rotation_enabled(&self) -> bool {
        self.automatic_rotation_limit != Some(0)
    }

    pub(crate) fn after_automatic_rotation(&self) -> Self {
        let mut next = self.clone();
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
        }
    }
}

/// Router-startup decision for the live upstream AlephBFT session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FederationAlephBftLiveSessionStartupDecision {
    /// The mint is not configured as a federation member.
    NoFederationConfig,
    /// Federation is configured with a different consensus backend.
    NonAlephBftBackend {
        /// Configured consensus backend.
        backend: FederationConsensusBackendKind,
    },
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
    let backend = config.consensus_backend();
    if backend != FederationConsensusBackendKind::AlephBft {
        return Ok(FederationAlephBftLiveSessionStartupDecision::NonAlephBftBackend { backend });
    }
    if state
        .federation_aleph_bft_live_session
        .read()
        .await
        .is_some()
        || state
            .federation_aleph_bft_network_inbound
            .read()
            .await
            .is_some()
    {
        return Ok(FederationAlephBftLiveSessionStartupDecision::AlreadyStarted);
    }

    let Some(consensus) = &state.federation_consensus else {
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
pub(crate) struct FederationAlephBftLiveSessionHandle {
    exit: Mutex<Option<oneshot::Sender<()>>>,
    activity_hint: CdkAlephBftActivityHint,
    spawn_handle: CdkAlephBftTokioSpawnHandle,
    session_task: JoinHandle<()>,
    network_pump_task: JoinHandle<()>,
    apply_task: JoinHandle<()>,
    completion_watcher_task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FederationAlephBftLiveSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationAlephBftLiveSessionHandle")
            .finish_non_exhaustive()
    }
}

impl FederationAlephBftLiveSessionHandle {
    pub(crate) fn mark_activity(&self) {
        self.activity_hint.mark_local_pending();
    }

    fn shutdown(&self) {
        self.shutdown_inner(true);
    }

    fn shutdown_for_rotation(&self) {
        self.shutdown_inner(false);
    }

    fn shutdown_inner(&self, abort_completion_watcher: bool) {
        if let Ok(mut exit) = self.exit.lock() {
            if let Some(exit) = exit.take() {
                let _ = exit.send(());
            }
        }
        self.spawn_handle.abort_all();
        self.network_pump_task.abort();
        self.apply_task.abort();
        self.session_task.abort();
        if abort_completion_watcher {
            if let Some(completion_watcher_task) = &self.completion_watcher_task {
                completion_watcher_task.abort();
            }
        }
    }
}

impl Drop for FederationAlephBftLiveSessionHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start the live upstream AlephBFT session and install its inbound network queue.
pub(crate) async fn start_federation_aleph_bft_live_session(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error> {
    start_federation_aleph_bft_live_session_with_config(
        state,
        sync_config,
        CdkAlephBftNetworkPumpConfig::default(),
    )
    .await
}

/// Start the live upstream AlephBFT session with explicit testable work bounds.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn start_federation_aleph_bft_live_session_with_config(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error> {
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
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
    transport: T,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    options: FederationAlephBftLiveSessionOptions,
    transport: T,
    restore_recovered_startup_round_cap: bool,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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

    let config =
        state.mint.federation_config().cloned().ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session missing config".to_string())
        })?;
    let public_config = config.public_config().clone();
    let private_config = config.private_config().clone();
    let keychain = CdkAlephBftKeychain::try_new(&public_config, private_config.clone())?;
    let consensus = state
        .federation_consensus
        .as_ref()
        .cloned()
        .ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session missing consensus runtime".to_string())
        })?;
    let retry_store = state.mint.localstore();
    let mut tx = retry_store.begin_transaction().await.map_err(|err| {
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
    let existing_session_backup_bytes = retry_store
        .get_federation_aleph_bft_backup_bytes(session_id)
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!(
                "AlephBFT network retry backup lookup failed: {err}"
            ))
        })?;
    let has_existing_session_backup = existing_session_backup_bytes
        .as_ref()
        .is_some_and(|bytes| !bytes.is_empty());
    let options = if restore_recovered_startup_round_cap {
        aleph_bft_live_session_options_for_startup(options, session_id, has_existing_session_backup)
    } else {
        options
    };
    let activity_hint = CdkAlephBftActivityHint::default();
    let session_io = CdkAlephBftSessionIo::from_db(
        Arc::clone(&consensus),
        Arc::clone(&retry_store),
        session_id,
        activity_hint.clone(),
    )
    .await?;
    let reload_persisted_network_retries = has_existing_session_backup;
    if !reload_persisted_network_retries {
        let orphan_retries = retry_store
            .get_federation_aleph_bft_network_retries(session_id)
            .await
            .map_err(|err| {
                cdk::Error::Custom(format!(
                    "AlephBFT orphan network retry lookup failed: {err}"
                ))
            })?;
        for retry in &orphan_retries {
            retry_store
                .remove_federation_aleph_bft_network_retry(&retry.retry_id)
                .await
                .map_err(|err| {
                    cdk::Error::Custom(format!(
                        "AlephBFT orphan network retry removal failed: {err}"
                    ))
                })?;
        }
        if !orphan_retries.is_empty() {
            tracing::warn!(
                session_id,
                orphan_retry_count = orphan_retries.len(),
                "Cleared AlephBFT network retries without a durable session backup"
            );
        }
    }
    let (local_io, finalized_units) = session_io.into_parts();
    let (network, mut network_handle) = CdkAlephBftNetwork::<CdkAlephBftNetworkData>::new_pair();
    let inbound = CdkAlephBftNetworkInboundHandle::with_activity_hint(
        session_id,
        &network_handle,
        activity_hint.clone(),
    );
    *state.federation_aleph_bft_network_inbound.write().await = Some(Arc::new(inbound));

    let aleph_config = aleph_bft_config(
        NodeCount(public_config.members.len()),
        keychain.index(),
        session_id,
        &options,
        activity_hint.clone(),
    )?;
    let (exit_sender, exit_receiver) = oneshot::channel();
    let terminator = Terminator::create_root(exit_receiver, "cdk-aleph-bft-session");
    let spawn_handle = CdkAlephBftTokioSpawnHandle::default();

    let session_keychain = keychain.clone();
    let session_spawn_handle = spawn_handle.clone();
    let (session_completed_sender, session_completed_receiver) = oneshot::channel();
    let session_task = tokio::spawn(async move {
        run_session(
            aleph_config,
            local_io,
            network,
            session_keychain,
            session_spawn_handle,
            terminator,
        )
        .await;
        let _ = session_completed_sender.send(());
    });

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
    let pump_keychain = keychain.clone();
    let network_pump_task = tokio::spawn(async move {
        let mut interval = federation_peer_sync_interval(sync_config);
        let mut retry_queue = if reload_persisted_network_retries {
            match CdkAlephBftNetworkRetryQueue::from_db(&retry_store, &pump_keychain, session_id)
                .await
            {
                Ok(queue) => queue,
                Err(err) => {
                    tracing::warn!("AlephBFT network retry reload failed: {}", err);
                    CdkAlephBftNetworkRetryQueue::default()
                }
            }
        } else {
            CdkAlephBftNetworkRetryQueue::default()
        };
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
                Some(&retry_store),
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
    });

    let apply_state = state.clone();
    let apply_task = tokio::spawn(async move {
        run_federation_aleph_bft_finalized_unit_apply_loop(apply_state, finalized_units).await;
    });
    let completion_watcher_task = spawn_federation_aleph_bft_session_completion_watcher(
        state.clone(),
        sync_config,
        options,
        rotation_transport,
        session_id,
        session_completed_receiver,
    );
    let handle = Arc::new(FederationAlephBftLiveSessionHandle {
        exit: Mutex::new(Some(exit_sender)),
        activity_hint,
        spawn_handle,
        session_task,
        network_pump_task,
        apply_task,
        completion_watcher_task,
    });
    *state.federation_aleph_bft_live_session.write().await = Some(Arc::clone(&handle));

    if state
        .federation_aleph_bft_status()
        .await
        .map(|status| status.finality)
        == Some(FederationAlephBftFinalityMode::ExternalFinalizationRequired)
    {
        state.mark_aleph_bft_network_finality_available().await?;
    }

    Ok(Some(handle))
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
        if round > CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND && !activity_hint.is_active() {
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
    session_completed_receiver: oneshot::Receiver<()>,
) -> Option<JoinHandle<()>>
where
    T: Transport + 'static,
{
    if !options.automatic_rotation_enabled() {
        return None;
    }

    Some(tokio::spawn(async move {
        match session_completed_receiver.await {
            Ok(_) => {
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
                }
            }
            Err(_) => {
                tracing::debug!(
                    completed_session,
                    "AlephBFT live session stopped before natural completion"
                );
            }
        }
    }))
}

/// Rotate a live upstream AlephBFT task into the next empty session.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn rotate_federation_aleph_bft_live_session_to_next_empty_with_transport<T>(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
    transport: T,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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
        .federation_aleph_bft_live_session
        .read()
        .await
        .is_none()
    {
        return Ok(None);
    }

    let Some(consensus) = state.federation_consensus.as_ref().cloned() else {
        return Err(cdk::Error::Custom(
            "AlephBFT live session rotation requires federation consensus runtime".to_string(),
        ));
    };

    let next_session = {
        let mut consensus = consensus.lock().await;
        let status = consensus.aleph_bft_status().map_err(cdk::Error::from)?;
        let pending = consensus.pending_mempool_len();
        if pending != 0 {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT live session rotation requires empty pending mempool, found {pending}"
            )));
        }
        let next_session = status.session.checked_add(1).ok_or_else(|| {
            cdk::Error::Custom("AlephBFT live session rotation overflowed session id".to_string())
        })?;
        consensus
            .set_aleph_bft_finality_mode(
                FederationAlephBftFinalityMode::ExternalFinalizationRequired,
            )
            .map_err(cdk::Error::from)?;
        next_session
    };

    let previous_handle = state.federation_aleph_bft_live_session.write().await.take();
    if let Some(handle) = previous_handle {
        handle.shutdown();
    }
    *state.federation_aleph_bft_network_inbound.write().await = None;

    {
        let mut consensus = consensus.lock().await;
        let pending = consensus.pending_mempool_len();
        if pending != 0 {
            return Err(cdk::Error::Custom(format!(
                "AlephBFT live session rotation requires empty pending mempool, found {pending}"
            )));
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
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error>
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
        .federation_aleph_bft_live_session
        .read()
        .await
        .is_none()
    {
        return Ok(None);
    }

    let Some(consensus) = state.federation_consensus.as_ref().cloned() else {
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
        consensus
            .set_aleph_bft_finality_mode(
                FederationAlephBftFinalityMode::ExternalFinalizationRequired,
            )
            .map_err(cdk::Error::from)?;
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

    let previous_handle = state.federation_aleph_bft_live_session.write().await.take();
    if let Some(handle) = previous_handle {
        handle.shutdown_for_rotation();
    }
    *state.federation_aleph_bft_network_inbound.write().await = None;

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
    mut finalized_units: tokio::sync::mpsc::UnboundedReceiver<
        cdk::federation::FederationAlephBftUnitPayload,
    >,
) {
    while let Some(payload) = finalized_units.recv().await {
        crate::federation::aleph_bft_latency::record(
            crate::federation::aleph_bft_latency::CdkAlephBftLatencyStage::ApplyLoopReceived,
            payload.operation_ids.iter().copied(),
        );
        apply_federation_aleph_bft_finalized_unit_with_retry(&state, payload).await;
    }
}

async fn apply_federation_aleph_bft_finalized_unit_with_retry(
    state: &MintState,
    payload: cdk::federation::FederationAlephBftUnitPayload,
) {
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
                return;
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
                tracing::warn!("AlephBFT finalized-unit apply loop failed: {}", err);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

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
            .with_automatic_rotation_limit(2);

        let first_rotation = options.after_automatic_rotation();
        assert_eq!(first_rotation.max_round(), 8);
        assert_eq!(first_rotation.automatic_rotation_limit(), Some(1));

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
        let early_delay =
            (delay_config.unit_creation_delay)(CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND);
        let tail_round = CDK_ALEPH_BFT_IDLE_SLOWDOWN_AFTER_ROUND + 1;

        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        );

        activity_hint.mark_local_pending();
        assert_eq!((delay_config.unit_creation_delay)(tail_round), early_delay);

        activity_hint.set_local_pending(false);
        assert_eq!(
            (delay_config.unit_creation_delay)(tail_round),
            CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY
        );

        activity_hint.mark_network_activity();
        assert_eq!((delay_config.unit_creation_delay)(tail_round), early_delay);
    }
}
