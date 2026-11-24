//! This node's side of the Bark driver port.

use std::sync::Arc;

use async_trait::async_trait;
use cdk::federation::{
    FederationBarkEvidenceAttestationRequest, FederationBarkPrepareReceiveSecretRequest,
    FederationBarkReceiveSecretAcknowledgement, FederationOperationEnvelope, FederationOperationId,
    FederationSubmitOperationResponse, MemberConfigPublic,
};
use cdk::mint::Mint;
use futures::future::BoxFuture;

use crate::peer_client::FederationRestPeerClient;
use crate::wallet_operation_publisher::{
    prior_publisher_wait, publisher_rank, wait_for_prior_publisher,
};
use crate::FederationNode;

/// Run the Bark recovery driver until the node stops, if Bark is configured.
pub fn spawn_bark_recovery(state: &FederationNode) {
    let Some(runtime) = state.federation.bark_runtime().cloned() else {
        return;
    };
    let tasks = state.federation.tasks.clone();
    let stop = tasks.stop_token();
    let host = Arc::new(cdk_bark::FederatedBarkNodeRecoveryHost::new(state.clone()));
    tasks.spawn(
        "Bark recovery",
        true,
        cdk_bark::FederatedBarkRecoveryDriver::new(runtime).run_until(host, stop.cancelled_owned()),
    );
}

#[async_trait]
impl cdk_bark::FederatedBarkNode for FederationNode {
    fn mint(&self) -> &Mint {
        &self.mint
    }

    fn bark_runtime(&self) -> Option<&Arc<cdk_bark::FederatedBarkRuntime>> {
        self.federation.bark_runtime()
    }

    fn bark_receive_secrets(&self) -> Option<Arc<cdk_bark::FederatedBarkReceiveSecretStore>> {
        self.federation.bark_receive_secrets()
    }

    fn bark_incoming_lock(&self) -> &tokio::sync::Mutex<()> {
        self.federation.bark_incoming_lock()
    }

    fn submit_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> BoxFuture<'_, Result<FederationSubmitOperationResponse, cdk::Error>> {
        self.submit_federation_envelope(envelope)
    }

    fn submit_ordered_successor_group(
        &self,
        envelopes: Vec<FederationOperationEnvelope>,
    ) -> BoxFuture<'_, Result<Vec<FederationSubmitOperationResponse>, cdk::Error>> {
        Box::pin(self.submit_federation_ordered_successor_group(envelopes))
    }

    async fn validate_envelope_admission(
        &self,
        envelope: &FederationOperationEnvelope,
    ) -> Result<(), cdk::Error> {
        self.validate_federation_envelope_admission(envelope).await
    }

    async fn ensure_ready_for_public_submit(&self) -> Result<(), cdk::Error> {
        self.ensure_federation_ready_for_public_submit().await
    }

    async fn wait_for_prior_wallet_publisher(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error> {
        let federation = self.mint.federation_config().ok_or(cdk::Error::Internal)?;
        let rank = publisher_rank(
            federation.public_config().members.keys().copied(),
            operation_id,
            federation.member_id(),
        )
        .ok_or(cdk::Error::Internal)?;
        let Some(delay) = prior_publisher_wait(rank) else {
            return Ok(None);
        };
        Ok(
            wait_for_prior_publisher(&self.federation, operation_id, delay)
                .await?
                .map(|response| *response),
        )
    }

    fn bark_peers(&self) -> Result<Box<dyn cdk_bark::FederatedBarkPeers + '_>, cdk::Error> {
        let federation = self.mint.federation_config().ok_or(cdk::Error::Internal)?;
        let client = FederationRestPeerClient::new_with_validated_config(
            self.federation.transport.clone(),
            federation.public_config(),
            federation.private_config().clone(),
            self.federation
                .peer_sync_config()
                .ok_or(cdk::Error::Internal)?,
        )
        .map_err(|_| cdk::Error::Internal)?;
        Ok(Box::new(client))
    }
}

#[async_trait]
impl<T> cdk_bark::FederatedBarkPeers for FederationRestPeerClient<T>
where
    T: cdk_http_client::Transport + Send + Sync,
{
    async fn prepare_bark_receive_secret(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkPrepareReceiveSecretRequest,
    ) -> anyhow::Result<FederationBarkReceiveSecretAcknowledgement> {
        FederationRestPeerClient::prepare_bark_receive_secret(self, member, request).await
    }

    async fn deliver_bark_evidence_attestation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkEvidenceAttestationRequest,
    ) -> anyhow::Result<()> {
        FederationRestPeerClient::deliver_bark_evidence_attestation(self, member, request)
            .await
            .map(drop)
    }
}
