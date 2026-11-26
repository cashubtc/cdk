//! Construction of a federation node from a mint and its member-local services.
//!
//! [`FederationNode::prepare`] builds the node runtime, resolves DKG
//! auto-start, recovers checkpoint candidates and restores startup readiness.
//! It starts no worker: the caller owns the returned [`FederationServices`]
//! and starts them when its listeners are ready.

use std::sync::Arc;

use anyhow::Result;
use cdk::mint::Mint;

use crate::runtime_components::FederationRuntimeComponents;
use crate::services::FederationServices;
#[cfg(feature = "federation-bdk")]
use crate::FederatedBdkLocalConfig;
use crate::{
    create_federation_runtime_state_with_wallet, default_federation_transport,
    federation_dkg_member_admission, FederationDkgAutoStartConfig, FederationMembership,
    FederationNode, FederationNodeRuntime, FederationPeerSyncConfig, FederationSyncState,
    FederationTransport,
};

/// Member-local services and policy a federation node is built with.
#[derive(Debug)]
pub struct FederationNodeConfig {
    /// Peer synchronization, admission, and retry policy.
    pub peer_sync: FederationPeerSyncConfig,
    /// Optional dealer-free DKG driver.
    pub dkg: Option<FederationDkgAutoStartConfig>,
    /// Optional watch-only Bitcoin wallet.
    #[cfg(feature = "federation-bdk")]
    pub bdk: Option<FederatedBdkLocalConfig>,
    /// Optional Bark custody runtime.
    #[cfg(feature = "federation-bark")]
    pub bark: Option<crate::FederatedBarkLocalConfig>,
    /// Shared outbound member transport.
    pub transport: FederationTransport,
}

impl Default for FederationNodeConfig {
    fn default() -> Self {
        Self {
            peer_sync: FederationPeerSyncConfig::default(),
            dkg: None,
            #[cfg(feature = "federation-bdk")]
            bdk: None,
            #[cfg(feature = "federation-bark")]
            bark: None,
            transport: default_federation_transport(),
        }
    }
}

/// What a constructed node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationNodeMode {
    /// A federation member: it synchronizes with its peers once its services
    /// start, and runs a configured DKG ceremony that has not completed yet.
    Member,
    /// A setup-only node that runs only the configured DKG ceremony.
    SetupDkg,
    /// A member without peer synchronization whose submissions finalize at once.
    #[cfg(any(test, feature = "test-utils"))]
    LocalFinality,
}

/// A constructed node and the workers it owns, before any worker starts.
#[allow(missing_debug_implementations)]
pub struct PreparedFederationNode {
    /// The node. Its mint is the one it was prepared for.
    pub node: FederationNode,
    /// The node's workers, present when the mint has a federation config.
    pub services: Option<FederationServices>,
    /// Whether the node serves the federation peer API: it is a consensus
    /// member or runs a DKG ceremony.
    pub serves_peer_api: bool,
}

impl FederationNode {
    /// Build the federation node for `mint`.
    ///
    /// A mint without a federation config yields a standalone node with no
    /// services. Nothing is spawned; start [`PreparedFederationNode::services`]
    /// to run the node's workers.
    pub async fn prepare(
        mint: Arc<Mint>,
        config: FederationNodeConfig,
        mode: FederationNodeMode,
    ) -> Result<PreparedFederationNode> {
        let FederationNodeConfig {
            peer_sync,
            dkg,
            #[cfg(feature = "federation-bdk")]
            bdk,
            #[cfg(feature = "federation-bark")]
            bark,
            transport,
        } = config;
        peer_sync.validate()?;
        let synchronizes = mode == FederationNodeMode::Member;
        #[cfg(feature = "prometheus")]
        let started_at = std::time::Instant::now();

        let runtime_state = create_federation_runtime_state_with_wallet(
            &mint,
            peer_sync,
            transport.clone(),
            #[cfg(feature = "federation-bdk")]
            bdk,
            #[cfg(feature = "federation-bark")]
            bark,
        )
        .await?;
        let sync = runtime_state
            .member
            .as_ref()
            .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::for_aleph_bft())));
        let mut node = FederationNode {
            mint: Arc::clone(&mint),
            federation: FederationNodeRuntime::new(
                transport,
                runtime_state.member.map_or(
                    FederationMembership::Standalone,
                    FederationMembership::configured,
                ),
                #[cfg(feature = "federation-bdk")]
                runtime_state.onchain_sync_interval,
                FederationRuntimeComponents::new(
                    sync,
                    None,
                    None,
                    #[cfg(feature = "federation-bdk")]
                    runtime_state.onchain_wallets,
                    runtime_state.frost_signing,
                    #[cfg(feature = "federation-bark")]
                    runtime_state.bark,
                ),
            ),
        };
        #[cfg(feature = "federation-bdk")]
        node.federation
            .components
            .set_startup_bdk_degraded_reason(runtime_state.bdk_degraded_reason)
            .await;
        #[cfg(any(test, feature = "test-utils"))]
        if mode == FederationNodeMode::LocalFinality {
            node.federation.finality = crate::FederationFinalityMode::Local;
            let sync = node
                .federation
                .consensus()
                .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::default())));
            node.federation.set_sync_handle(sync);
        }

        let dkg = match dkg {
            Some(dkg) => {
                let runtime_config = mint.federation_config().ok_or_else(|| {
                    anyhow::anyhow!(
                        "federation DKG auto-start requires a mint federation runtime config"
                    )
                })?;
                let public_config = runtime_config.public_config();
                if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
                    tracing::info!(
                        member_id = %runtime_config.member_id(),
                        ceremony_id = %dkg.ceremony_id,
                        "Federation DKG auto-start skipped because finalized key material is already active"
                    );
                    None
                } else {
                    dkg.validate_for_public_config(public_config)?;
                    Some(dkg)
                }
            }
            None => None,
        };
        let dkg_member_admission = dkg.as_ref().and_then(|dkg| {
            mint.federation_config().map(|runtime_config| {
                federation_dkg_member_admission(
                    runtime_config.public_config(),
                    dkg.driver.member_request_burst,
                    dkg.driver.member_request_refill_interval,
                )
            })
        });
        node.federation
            .set_dkg_components(dkg.clone().map(Arc::new), dkg_member_admission);

        if node.federation.consensus().is_some() {
            node.federation
                .recover_scheduled_checkpoint_candidate_at_startup()
                .await?;
            node.federation
                .recover_recovery_checkpoint_candidate_at_startup()
                .await?;
        }
        if synchronizes {
            node.restore_federation_startup_readiness().await;
        }
        #[cfg(feature = "prometheus")]
        if let Some(config) = mint.federation_config() {
            cdk_prometheus::METRICS.record_federation_checkpoint_duration(
                &config.member_id().to_string(),
                cdk_prometheus::FederationCheckpointOperation::StartupReadiness,
                started_at.elapsed().as_secs_f64(),
            );
        }

        let serves_peer_api = node.federation.consensus().is_some() || dkg.is_some();
        let services = mint
            .federation_config()
            .map(|_| FederationServices::new(node.clone(), synchronizes.then_some(peer_sync), dkg));
        Ok(PreparedFederationNode {
            node,
            services,
            serves_peer_api,
        })
    }
}
