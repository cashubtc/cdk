//! Resume logic for receive sagas after crash recovery.
//!
//! This module handles resuming incomplete receive sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.

use cdk_common::wallet::{OperationData, ReceiveOperationData, ReceiveSagaState, WalletSaga};
use tracing::instrument;

use crate::wallet::receive::saga::compensation::RemovePendingProofs;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete receive saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **ProofsPending**: Proofs stored in Pending state but swap not executed.
    ///   Safe to compensate by removing the pending proofs.
    ///
    /// - **SwapRequested**: Swap was requested. Check if input proofs are spent.
    ///   If spent, try to reconstruct outputs. If not spent, compensate.
    #[instrument(skip(self, saga))]
    pub async fn resume_receive_saga(&self, saga: &WalletSaga) -> Result<RecoveryAction, Error> {
        let state = match &saga.state {
            cdk_common::wallet::WalletSagaState::Receive(s) => s,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for receive saga {}",
                    saga.id
                )))
            }
        };

        let data = match &saga.data {
            OperationData::Receive(d) => d,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for receive saga {}",
                    saga.id
                )))
            }
        };

        match state {
            ReceiveSagaState::ProofsPending => {
                // No swap was executed - safe to compensate by removing pending proofs
                tracing::info!(
                    "Receive saga {} in ProofsPending state - compensating",
                    saga.id
                );
                self.compensate_receive(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            ReceiveSagaState::SwapRequested => {
                // Similar to swap saga - check if proofs were spent
                tracing::info!(
                    "Receive saga {} in SwapRequested state - checking mint for proof states",
                    saga.id
                );
                self.recover_or_compensate_receive(&saga.id, data).await
            }
        }
    }

    /// Check mint and either complete receive or compensate.
    async fn recover_or_compensate_receive(
        &self,
        saga_id: &uuid::Uuid,
        data: &ReceiveOperationData,
    ) -> Result<RecoveryAction, Error> {
        // Get the pending proofs for this specific operation
        let pending_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if pending_proofs.is_empty() {
            tracing::warn!(
                "No pending proofs found for receive saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        // Check proof states with the mint
        match self.are_proofs_spent(&pending_proofs).await {
            Ok(true) => {
                // Input proofs are spent - try to recover outputs
                tracing::info!(
                    "Receive saga {} - input proofs spent, recovering outputs",
                    saga_id
                );
                self.complete_receive_from_restore(saga_id, data, &pending_proofs)
                    .await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
                // Proofs not spent - swap failed, compensate
                tracing::info!(
                    "Receive saga {} - input proofs not spent, compensating",
                    saga_id
                );
                self.compensate_receive(saga_id).await?;
                Ok(RecoveryAction::Compensated)
            }
            Err(e) => {
                tracing::warn!(
                    "Receive saga {} - can't check proof states ({}), skipping",
                    saga_id,
                    e
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Complete a receive by restoring outputs from the mint.
    async fn complete_receive_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &ReceiveOperationData,
        pending_proofs: &[crate::types::ProofInfo],
    ) -> Result<(), Error> {
        // Try to restore outputs using stored blinded messages
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Receive",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        // Remove the input proofs (they're spent) and add recovered proofs
        let input_ys: Vec<_> = pending_proofs.iter().map(|p| p.y).collect();

        match new_proofs {
            Some(proofs) => {
                self.localstore.update_proofs(proofs, input_ys).await?;
            }
            None => {
                // Couldn't restore outputs - just remove the spent inputs
                tracing::warn!(
                    "Receive saga {} - couldn't restore outputs, removing spent inputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_id
                );
                self.localstore.update_proofs(vec![], input_ys).await?;
            }
        }

        // Delete the saga record
        self.localstore.delete_saga(saga_id).await?;

        Ok(())
    }

    /// Compensate a receive saga by removing pending proofs.
    async fn compensate_receive(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
        let pending_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        let proof_ys = pending_proofs.iter().map(|p| p.y).collect();

        RemovePendingProofs {
            localstore: self.localstore.clone(),
            proof_ys,
            saga_id: *saga_id,
        }
        .execute()
        .await
    }
}

#[cfg(test)]
mod tests {
    // Tests will be moved here from recovery.rs
}
