use std::sync::{Arc, Mutex};
use std::time::Duration;

use aleph_bft::{default_config, run_session, Index, NodeCount, Terminator};
use cdk::federation::{FederationAlephBftFinalityMode, FederationConsensusBackendKind, MemberId};
use cdk_http_client::Async;
use futures::channel::oneshot;
use tokio::task::JoinHandle;

use crate::MintState;

use super::aleph_bft_network::{
    run_cdk_aleph_bft_network_pump_tick, CdkAlephBftNetworkInboundHandle,
    CdkAlephBftNetworkPumpConfig,
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

impl Drop for FederationAlephBftLiveSessionHandle {
    fn drop(&mut self) {
        if let Ok(mut exit) = self.exit.lock() {
            if let Some(exit) = exit.take() {
                let _ = exit.send(());
            }
        }
        self.network_pump_task.abort();
        self.apply_task.abort();
        self.session_task.abort();
    }
}

/// Start the live upstream AlephBFT session and install its inbound network queue.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn start_federation_aleph_bft_live_session_with_config(
    state: &mut MintState,
    sync_config: FederationPeerSyncConfig,
    pump_config: CdkAlephBftNetworkPumpConfig,
) -> Result<Option<Arc<FederationAlephBftLiveSessionHandle>>, cdk::Error> {
    sync_config.validate().map_err(|err| {
        cdk::Error::Custom(format!("Invalid AlephBFT live session sync config: {err}"))
    })?;
    pump_config.validate()?;

    let decision = federation_aleph_bft_live_session_startup_decision(state).await?;
    let FederationAlephBftLiveSessionStartupDecision::Spawn { session_id, .. } = decision else {
        return Ok(None);
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
    let session_io =
        CdkAlephBftSessionIo::from_db(Arc::clone(&consensus), state.mint.localstore(), session_id)
            .await?;
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

    let session_keychain = keychain.clone();
    let session_task = tokio::spawn(async move {
        run_session(
            aleph_config,
            local_io,
            network,
            session_keychain,
            CdkAlephBftTokioSpawnHandle,
            terminator,
        )
        .await;
    });

    let client = FederationRestPeerClient::new_with_config(
        Async::default(),
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
        loop {
            interval.tick().await;
            match run_cdk_aleph_bft_network_pump_tick(
                &mut network_handle,
                &pump_keychain,
                &client,
                pump_config,
            )
            .await
            {
                Ok(outcome) => {
                    if outcome.messages_drained > 0 {
                        tracing::debug!(
                            messages_drained = outcome.messages_drained,
                            encoded_bytes = outcome.encoded_bytes,
                            remote_accepted_members = outcome.remote_accepted_members.len(),
                            remote_failed_members = outcome.remote_failed_members.len(),
                            remote_skipped_members = outcome.remote_skipped_members.len(),
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
