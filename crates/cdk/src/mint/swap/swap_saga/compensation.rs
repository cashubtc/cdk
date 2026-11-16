use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::{DhkePoint, Error, ProofId, State};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::subscription::PubSubManager;

#[async_trait]
pub trait CompensatingAction: Send + Sync {
    async fn execute(&self, db: &DynMintDatabase, pubsub: &PubSubManager) -> Result<(), Error>;
    fn name(&self) -> &'static str;
}

/// Compensation action to remove swap setup (both proofs and blinded messages).
///
/// This compensation is used when blind signing fails or finalization fails after
/// the setup transaction has committed. It removes:
/// - Output blinded messages (identified by blinded_secrets)
/// - Input proofs (identified by input_ys)
/// - Saga state record
///
/// This restores the database to its pre-swap state and publishes the inputs
/// as unspent again.
pub struct RemoveSwapSetup {
    /// Blinded secrets (B values) from the output blinded messages
    pub blinded_secrets: Vec<DhkePoint>,
    /// Y values (public keys) from the input proofs
    pub input_ys: Vec<ProofId>,
    /// Operation ID (saga ID) to delete
    pub operation_id: Uuid,
}

#[async_trait]
impl CompensatingAction for RemoveSwapSetup {
    #[instrument(skip_all)]
    async fn execute(&self, db: &DynMintDatabase, pubsub: &PubSubManager) -> Result<(), Error> {
        if self.blinded_secrets.is_empty() && self.input_ys.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "Compensation: Removing swap setup ({} blinded messages, {} proofs, saga {})",
            self.blinded_secrets.len(),
            self.input_ys.len(),
            self.operation_id
        );

        let mut tx = db.begin_transaction().await?;

        // Remove blinded messages (outputs)
        if !self.blinded_secrets.is_empty() {
            tx.delete_blinded_messages(&self.blinded_secrets).await?;
        }

        // Remove proofs (inputs)
        if !self.input_ys.is_empty() {
            tx.remove_proofs(&self.input_ys, None).await?;
        }

        // Delete saga state record
        if let Err(e) = tx.delete_saga(&self.operation_id).await {
            tracing::warn!(
                "Failed to delete saga {} during compensation: {}",
                self.operation_id,
                e
            );
            // Continue anyway - saga cleanup is best-effort
        }

        tx.commit().await?;

        // Setup published the inputs as pending; tell subscribers they are
        // spendable again now that the swap is rolled back
        for y in &self.input_ys {
            pubsub.proof_state((*y, State::Unspent));
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "RemoveSwapSetup"
    }
}
