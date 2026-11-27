//! Bark payment drivers running on a federation node.
//!
//! The drivers own Bark phase selection, evidence collection, replay and the
//! construction of every Bark operation. A federation node runs them through
//! [`FederatedBarkNode`], which supplies only what a node has: its mint,
//! consensus submission, wallet-publisher ordering and authenticated peer
//! delivery.

use std::sync::Arc;

use async_trait::async_trait;
use cdk::federation::{
    latency, FederationBarkEvidenceAttestationRequest, FederationBarkOperation,
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveSecretAcknowledgement,
    FederationConfig, FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationSubmitOperationResponse, MemberConfigPublic,
};
use cdk::mint::Mint;
use futures::future::BoxFuture;

use crate::{FederatedBarkReceiveSecretStore, FederatedBarkRuntime};

mod custody_attempt;
mod evidence;
mod incoming;
mod maintenance;
mod observer;
mod outgoing;
mod projection;
mod receive_secret;

pub use self::incoming::{
    advance_federated_bark_mint_quote, advance_federated_bark_mint_quote_from_evidence_delivery,
    advance_federated_bark_receive_intent, federated_bark_quote_id,
    prepare_federated_bark_mint_quote,
};
pub use self::maintenance::{
    retry_federated_bark_maintenance_action, FederatedBarkNodeRecoveryHost,
};
pub use self::outgoing::{
    advance_federated_bark_melt_quote, advance_federated_bark_melt_quote_from_evidence_delivery,
    submit_federated_bark_melt_with_plan,
};
pub use self::projection::{
    federation_bark_ready_indexed_projection, federation_bark_ready_projection,
    federation_bark_recovery_indexed_projection, federation_bark_recovery_projection,
    reconcile_federation_bark_runtime,
};

/// Federation node capabilities the Bark payment drivers use.
#[async_trait]
pub trait FederatedBarkNode: Send + Sync {
    /// Mint whose accepted federation state the drivers read.
    fn mint(&self) -> &Mint;

    /// This member's Bark runtime, when Bark custody is configured.
    fn bark_runtime(&self) -> Option<&Arc<FederatedBarkRuntime>>;

    /// This member's protected receive-secret store, when Bark is configured.
    fn bark_receive_secrets(&self) -> Option<Arc<FederatedBarkReceiveSecretStore>>;

    /// Lock serializing this member's incoming Bark work.
    fn bark_incoming_lock(&self) -> &tokio::sync::Mutex<()>;

    /// Submit one envelope and wait for its consensus outcome.
    fn submit_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> BoxFuture<'_, Result<FederationSubmitOperationResponse, cdk::Error>>;

    /// Submit envelopes that consensus must order in one unit, in this order.
    fn submit_ordered_successor_group(
        &self,
        envelopes: Vec<FederationOperationEnvelope>,
    ) -> BoxFuture<'_, Result<Vec<FederationSubmitOperationResponse>, cdk::Error>>;

    /// Check that consensus would admit a wallet-authored envelope.
    async fn validate_envelope_admission(
        &self,
        envelope: &FederationOperationEnvelope,
    ) -> Result<(), cdk::Error>;

    /// Fail unless this member may accept public submissions.
    async fn ensure_ready_for_public_submit(&self) -> Result<(), cdk::Error>;

    /// Wait for this member's turn to publish a wallet-fanned operation.
    ///
    /// Returns the outcome when a member ahead in the publisher order got the
    /// identical operation accepted while this member waited.
    async fn wait_for_prior_wallet_publisher(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationSubmitOperationResponse>, cdk::Error>;

    /// Authenticated client for Bark requests to other members.
    fn bark_peers(&self) -> Result<Box<dyn FederatedBarkPeers + '_>, cdk::Error>;
}

/// Authenticated Bark requests to other federation members.
#[async_trait]
pub trait FederatedBarkPeers: Send + Sync {
    /// Ask one member to store a protected receive secret and acknowledge it.
    async fn prepare_bark_receive_secret(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkPrepareReceiveSecretRequest,
    ) -> anyhow::Result<FederationBarkReceiveSecretAcknowledgement>;

    /// Deliver this member's evidence attestation to a collector.
    async fn deliver_bark_evidence_attestation(
        &self,
        member: &MemberConfigPublic,
        request: &FederationBarkEvidenceAttestationRequest,
    ) -> anyhow::Result<()>;
}

/// Decode an accepted operation as a Bark operation.
pub fn bark_operation(
    operation: &FederationOperation,
) -> Result<FederationBarkOperation, cdk::Error> {
    cdk::federation::validate_federation_bark_operation(operation).map_err(cdk::Error::from)
}

/// Wrap a Bark operation as a federation application operation.
pub fn bark_federation_operation(
    operation: FederationBarkOperation,
) -> Result<FederationOperation, cdk::Error> {
    cdk::federation::federation_bark_application_operation(operation).map_err(cdk::Error::from)
}

/// Bark application configuration of a federation.
pub fn bark_application(
    config: &FederationConfig,
) -> Result<cdk::federation::FederationBarkFrostApplicationConfig, cdk::Error> {
    cdk::federation::federation_bark_application(config)?.ok_or(cdk::Error::Internal)
}
