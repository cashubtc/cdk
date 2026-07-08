use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::{default_config, run_session, Index, NodeCount, Terminator};
use cdk::federation::{FederationAlephBftFinalityMode, FederationConsensusBackendKind, MemberId};
use cdk_http_client::{Async, Transport};
use futures::channel::oneshot;
use tokio::task::JoinHandle;

use crate::MintState;

use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick_with_retry_queue, CdkAlephBftNetworkInboundHandle,
    CdkAlephBftNetworkPumpConfig, CdkAlephBftNetworkRetryQueue,
};
use super::aleph_bft_runtime::{
    CdkAlephBftKeychain, CdkAlephBftNetwork, CdkAlephBftNetworkData, CdkAlephBftSessionIo,
    CdkAlephBftTokioSpawnHandle,
};
use super::peer_client::FederationRestPeerClient;
use super::sync_config::{federation_peer_sync_interval, FederationPeerSyncConfig};

// AlephBFT validates its default delay schedule by summing all configured
// round delays. Keep the first live-session cap comfortably bounded until
// session rotation/checkpointing is wired as a deliberate follow-up.
const CDK_ALEPH_BFT_MAX_ROUND: u16 = 3000;

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
    if state.federation_aleph_bft_live_session.is_some()
        || state.federation_aleph_bft_network_inbound.is_some()
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
    spawn_handle: CdkAlephBftTokioSpawnHandle,
    session_task: JoinHandle<()>,
    network_pump_task: JoinHandle<()>,
    apply_task: JoinHandle<()>,
}

impl std::fmt::Debug for FederationAlephBftLiveSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationAlephBftLiveSessionHandle")
            .finish_non_exhaustive()
    }
}

impl FederationAlephBftLiveSessionHandle {
    fn shutdown(&self) {
        if let Ok(mut exit) = self.exit.lock() {
            if let Some(exit) = exit.take() {
                let _ = exit.send(());
            }
        }
        self.spawn_handle.abort_all();
        self.network_pump_task.abort();
        self.apply_task.abort();
        self.session_task.abort();
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
    start_federation_aleph_bft_live_session_with_transport(
        state,
        sync_config,
        pump_config,
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
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!("Invalid AlephBFT live session sync config: {err}"))
    })?;
    pump_config.validate()?;

    let decision = federation_aleph_bft_live_session_startup_decision(state).await?;
    let FederationAlephBftLiveSessionStartupDecision::Spawn { session_id, .. } = decision else {
        return Ok(None);
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id,
        ..pump_config
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
    let session_io =
        CdkAlephBftSessionIo::from_db(Arc::clone(&consensus), Arc::clone(&retry_store), session_id)
            .await?;
    let reload_persisted_network_retries = retry_store
        .get_federation_aleph_bft_backup_bytes(session_id)
        .await
        .map_err(|err| {
            cdk::Error::Custom(format!(
                "AlephBFT network retry backup lookup failed: {err}"
            ))
        })?
        .is_some_and(|bytes| !bytes.is_empty());
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
    let inbound = CdkAlephBftNetworkInboundHandle::new(session_id, &network_handle);
    state.federation_aleph_bft_network_inbound = Some(Arc::new(inbound));

    let aleph_config = default_config(
        NodeCount(public_config.members.len()),
        keychain.index(),
        session_id,
        CDK_ALEPH_BFT_MAX_ROUND,
        Duration::from_secs(0),
    )
    .map_err(|_| cdk::Error::Custom("Invalid AlephBFT live session config".to_string()))?;
    let (exit_sender, exit_receiver) = oneshot::channel();
    let terminator = Terminator::create_root(exit_receiver, "cdk-aleph-bft-session");
    let spawn_handle = CdkAlephBftTokioSpawnHandle::default();

    let session_keychain = keychain.clone();
    let session_spawn_handle = spawn_handle.clone();
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
    });

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
        interval.tick().await;
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
            interval.tick().await;
            match run_cdk_aleph_bft_network_pump_tick_with_retry_queue(
                &mut network_handle,
                &pump_keychain,
                &client,
                pump_config,
                &mut retry_queue,
                Some(&retry_store),
            )
            .await
            {
                Ok(outcome) => {
                    if outcome.messages_drained > 0 || outcome.retry_messages_drained > 0 {
                        tracing::debug!(
                            messages_drained = outcome.messages_drained,
                            retry_messages_drained = outcome.retry_messages_drained,
                            encoded_bytes = outcome.encoded_bytes,
                            remote_accepted_members = outcome.remote_accepted_members.len(),
                            remote_failed_members = outcome.remote_failed_members.len(),
                            remote_requeued_members = outcome.remote_requeued_members.len(),
                            remote_skipped_members = outcome.remote_skipped_members.len(),
                            retry_queue_len = outcome.retry_queue_len,
                            reached_idle = outcome.reached_idle,
                            "AlephBFT network pump tick completed"
                        );
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
    let handle = Arc::new(FederationAlephBftLiveSessionHandle {
        exit: Mutex::new(Some(exit_sender)),
        spawn_handle,
        session_task,
        network_pump_task,
        apply_task,
    });
    state.federation_aleph_bft_live_session = Some(Arc::clone(&handle));

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
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!(
            "Invalid AlephBFT live session rotation sync config: {err}"
        ))
    })?;
    pump_config.validate()?;

    if state.federation_aleph_bft_live_session.is_none() {
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

    if let Some(handle) = state.federation_aleph_bft_live_session.take() {
        handle.shutdown();
    }
    state.federation_aleph_bft_network_inbound = None;

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

    match start_federation_aleph_bft_live_session_with_transport(
        state,
        sync_config,
        pump_config,
        transport,
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
        if let Err(err) = state
            .apply_federation_aleph_bft_finalized_unit(payload)
            .await
        {
            tracing::warn!("AlephBFT finalized-unit apply loop failed: {}", err);
        }
    }
}
