use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::{Error, PublicKey, State};
use tracing::instrument;
use uuid::Uuid;

use super::SWAP_RECOVERY_LEASE;
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
    pub blinded_secrets: Vec<PublicKey>,
    /// Y values (public keys) from the input proofs
    pub input_ys: Vec<PublicKey>,
    /// Operation ID (saga ID) to delete
    pub operation_id: Uuid,
}

impl RemoveSwapSetup {
    pub(in crate::mint) async fn recover(
        &self,
        db: &DynMintDatabase,
        pubsub: &PubSubManager,
    ) -> Result<bool, Error> {
        self.remove(db, pubsub, true).await
    }

    #[instrument(skip_all)]
    async fn remove(
        &self,
        db: &DynMintDatabase,
        pubsub: &PubSubManager,
        recovering: bool,
    ) -> Result<bool, Error> {
        let mut tx = db.begin_transaction().await?;
        let Some(saga) = tx.get_saga_for_update(&self.operation_id).await? else {
            tx.rollback().await?;
            return Ok(false);
        };
        if recovering
            && db.uses_swap_recovery_leases()
            && tx.melt_lease_time().await?
                < saga
                    .updated_at
                    .saturating_add(SWAP_RECOVERY_LEASE.as_secs())
        {
            tx.rollback().await?;
            return Ok(false);
        }

        tracing::info!(
            "Compensation: Removing swap setup ({} blinded messages, {} proofs, saga {})",
            self.blinded_secrets.len(),
            self.input_ys.len(),
            self.operation_id
        );

        // Remove blinded messages (outputs)
        if !self.blinded_secrets.is_empty() {
            tx.delete_blinded_messages(&self.blinded_secrets).await?;
        }

        // Remove proofs (inputs)
        if !self.input_ys.is_empty() {
            tx.remove_proofs(&self.input_ys, None).await?;
        }

        tx.delete_saga(&self.operation_id).await?;
        tx.commit().await?;

        // Setup published the inputs as pending; tell subscribers they are
        // spendable again now that the swap is rolled back
        for y in &self.input_ys {
            pubsub.proof_state((*y, State::Unspent));
        }

        Ok(true)
    }
}

#[async_trait]
impl CompensatingAction for RemoveSwapSetup {
    async fn execute(&self, db: &DynMintDatabase, pubsub: &PubSubManager) -> Result<(), Error> {
        self.remove(db, pubsub, false).await?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "RemoveSwapSetup"
    }
}
