//! Resume logic for issue (mint) sagas after crash recovery.
//!
//! This module handles resuming incomplete issue sagas that were interrupted
//! by a crash. It attempts to recover outputs using stored blinded messages.

use cdk_common::wallet::{IssueSagaState, MintOperationData, OperationData, WalletSaga};
use tracing::instrument;

use crate::wallet::issue::saga::compensation::ReleaseMintQuote;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete issue (mint) saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **SecretsPrepared**: Secrets created but mint request not sent.
    ///   Safe to compensate (no proofs to revert, just release quote and delete saga).
    ///
    /// - **MintRequested**: Mint request was sent. Try to recover outputs
    ///   using stored blinded messages.
    #[instrument(skip(self, saga))]
    pub async fn resume_issue_saga(&self, saga: &WalletSaga) -> Result<RecoveryAction, Error> {
        let state = match &saga.state {
            cdk_common::wallet::WalletSagaState::Issue(s) => s,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for issue saga {}",
                    saga.id
                )))
            }
        };

        let data = match &saga.data {
            OperationData::Mint(d) => d,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for issue saga {}",
                    saga.id
                )))
            }
        };

        match state {
            IssueSagaState::SecretsPrepared => {
                // No mint request was sent - safe to delete saga
                // Counter increments are not reversed (by design)
                tracing::info!(
                    "Issue saga {} in SecretsPrepared state - cleaning up",
                    saga.id
                );
                self.compensate_issue(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            IssueSagaState::MintRequested => {
                // Mint request was sent - try to recover outputs
                tracing::info!(
                    "Issue saga {} in MintRequested state - attempting recovery",
                    saga.id
                );
                self.complete_issue_from_restore(&saga.id, data).await?;
                Ok(RecoveryAction::Recovered)
            }
        }
    }

    /// Complete an issue by restoring outputs from the mint.
    async fn complete_issue_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<(), Error> {
        // Try to restore outputs using stored blinded messages
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Issue",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        match new_proofs {
            Some(proofs) => {
                // Issue has no input proofs to remove - just add the recovered proofs
                self.localstore.update_proofs(proofs, vec![]).await?;
            }
            None => {
                // Couldn't restore outputs - issue saga has no inputs to mark spent
                tracing::warn!(
                    "Issue saga {} - couldn't restore outputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_id
                );
            }
        }

        // Delete the saga record
        self.localstore.delete_saga(saga_id).await?;

        Ok(())
    }

    /// Compensate an issue saga by releasing the quote and deleting the saga.
    async fn compensate_issue(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
        // Release the mint quote reservation (best-effort, continue on error)
        if let Err(e) = (ReleaseMintQuote {
            localstore: self.localstore.clone(),
            operation_id: *saga_id,
        }
        .execute()
        .await)
        {
            tracing::warn!(
                "Failed to release mint quote for saga {}: {}. Continuing with saga cleanup.",
                saga_id,
                e
            );
        }

        self.localstore.delete_saga(saga_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests will be moved here from recovery.rs
}
