//! Federation node runtime ownership.

use std::sync::Arc;

use super::{FederationDkgAutoStartConfig, FederationPeerSyncConfig};
use crate::{
    FederationAlephBftLiveSessionState, FederationAlephBftNetworkInboundState,
    FederationAlephBftPendingSequenceHandle, FederationConsensusHandle,
    FederationDkgMemberAdmissionHandle, FederationInvalidOperationAlertHandle,
    FederationNotificationHandle, FederationOperationNotificationStats,
    FederationRestAdmissionHandle, FederationSequenceHandle, FederationSyncHandle,
};
use cdk::federation::{FederationOperationId, FederationSubmitOperationResponse};

/// Runtime-owned handles for one active federation member.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub(crate) struct FederationNodeRuntime {
    pub(crate) consensus: Option<FederationConsensusHandle>,
    pub(crate) peer_sync_config: Option<FederationPeerSyncConfig>,
    pub(crate) dkg_auto_start_config: Option<Arc<FederationDkgAutoStartConfig>>,
    pub(crate) sequences: Option<FederationSequenceHandle>,
    pub(crate) sync: Option<FederationSyncHandle>,
    pub(crate) notifications: Option<FederationNotificationHandle>,
    pub(crate) private_rest_admission: Option<FederationRestAdmissionHandle>,
    pub(crate) dkg_member_admission: Option<FederationDkgMemberAdmissionHandle>,
    pub(crate) invalid_operation_alerts: Option<FederationInvalidOperationAlertHandle>,
    pub(crate) aleph_bft_pending_sequences: Option<FederationAlephBftPendingSequenceHandle>,
    pub(crate) aleph_bft_network_inbound: FederationAlephBftNetworkInboundState,
    pub(crate) aleph_bft_live_session: FederationAlephBftLiveSessionState,
    #[cfg(test)]
    pub(crate) local_finality_test: bool,
    #[cfg(test)]
    pub(crate) manual_finality_test: bool,
}

impl FederationNodeRuntime {
    /// Build the inert runtime stored by a standalone mint in a federation-enabled binary.
    pub(crate) fn disabled() -> Self {
        Self {
            consensus: None,
            peer_sync_config: None,
            dkg_auto_start_config: None,
            sequences: None,
            sync: None,
            notifications: None,
            private_rest_admission: None,
            dkg_member_admission: None,
            invalid_operation_alerts: None,
            aleph_bft_pending_sequences: None,
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            #[cfg(test)]
            local_finality_test: false,
            #[cfg(test)]
            manual_finality_test: false,
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_operation(
        &self,
        operation_id: FederationOperationId,
        waiter_limit: usize,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        let Some(notifications) = self.notifications.as_ref() else {
            return Ok(None);
        };
        Ok(Some(
            notifications.wait_for(operation_id, waiter_limit).await?,
        ))
    }

    pub(crate) fn pending_response_waiter_count(&self) -> usize {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.pending_waiter_count())
            .unwrap_or_default()
    }

    pub(crate) fn response_cache_entry_count(&self) -> usize {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.response_cache_count())
            .unwrap_or_default()
    }

    pub(crate) fn operation_notification_stats(&self) -> FederationOperationNotificationStats {
        self.notifications
            .as_ref()
            .map(|notifications| notifications.stats())
            .unwrap_or_default()
    }

    pub(crate) fn notify_operation(&self, response: FederationSubmitOperationResponse) {
        let Some(notifications) = &self.notifications else {
            return;
        };
        super::aleph_bft_latency::record(
            super::aleph_bft_latency::CdkAlephBftLatencyStage::OperationNotified,
            [response.operation_id],
        );
        let response_cache_limit = self
            .peer_sync_config
            .unwrap_or_default()
            .response_cache_limit;
        notifications.notify(response, response_cache_limit);
    }
}
