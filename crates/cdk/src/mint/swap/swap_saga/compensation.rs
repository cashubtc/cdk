use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::{Error, PublicKey};
use tracing::instrument;

#[async_trait]
pub trait CompensatingAction: Send + Sync {
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error>;
    fn name(&self) -> &'static str;
}

/// Compensation action to remove swap setup (both proofs and blinded messages)
/// This is used when blind signing fails or finalization fails
pub struct RemoveSwapSetup {
    pub secrets: Vec<PublicKey>,
    pub ys: Vec<PublicKey>,
}

#[async_trait]
impl CompensatingAction for RemoveSwapSetup {
    #[instrument(skip_all)]
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error> {
        if self.secrets.is_empty() && self.ys.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "Compensation: Removing swap setup ({} blinded messages, {} proofs)",
            self.secrets.len(),
            self.ys.len()
        );

        let mut tx = db.begin_transaction().await?;

        // Remove blinded messages (outputs)
        if !self.secrets.is_empty() {
            tx.delete_blinded_messages(&self.secrets).await?;
        }

        // Remove proofs (inputs)
        if !self.ys.is_empty() {
            tx.remove_proofs(&self.ys, None).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "RemoveSwapSetup"
    }
}
