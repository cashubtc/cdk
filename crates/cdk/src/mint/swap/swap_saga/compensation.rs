use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::{Error, PublicKey};
use tracing::instrument;

#[async_trait]
pub trait CompensatingAction: Send + Sync {
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error>;
    fn name(&self) -> &'static str;
}

/// Compensation action to remove swap setup (both proofs and blinded messages).
///
/// This compensation is used when blind signing fails or finalization fails after
/// the setup transaction has committed. It removes:
/// - Output blinded messages (identified by blinded_secrets)
/// - Input proofs (identified by input_ys)
///
/// This restores the database to its pre-swap state.
pub struct RemoveSwapSetup {
    /// Blinded secrets (B values) from the output blinded messages
    pub blinded_secrets: Vec<PublicKey>,
    /// Y values (public keys) from the input proofs
    pub input_ys: Vec<PublicKey>,
}

#[async_trait]
impl CompensatingAction for RemoveSwapSetup {
    #[instrument(skip_all)]
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error> {
        if self.blinded_secrets.is_empty() && self.input_ys.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "Compensation: Removing swap setup ({} blinded messages, {} proofs)",
            self.blinded_secrets.len(),
            self.input_ys.len()
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

        tx.commit().await?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "RemoveSwapSetup"
    }
}
