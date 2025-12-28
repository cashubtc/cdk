//! Resume logic for swap sagas after crash recovery.
//!
//! This module handles resuming incomplete swap sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.

use cdk_common::wallet::{OperationData, SwapOperationData, SwapSagaState, WalletSaga};
use tracing::instrument;

use crate::nuts::State;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::{CompensatingAction, RevertProofReservation};
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete swap saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **ProofsReserved**: The swap request hasn't been sent to the mint yet.
    ///   Safe to compensate by releasing the reserved proofs.
    ///
    /// - **SwapRequested**: The swap request was sent but we don't know the outcome.
    ///   Check the mint to determine if the swap succeeded, then either
    ///   complete the operation or compensate.
    #[instrument(skip(self, saga))]
    pub async fn resume_swap_saga(&self, saga: &WalletSaga) -> Result<RecoveryAction, Error> {
        let state = match &saga.state {
            cdk_common::wallet::WalletSagaState::Swap(s) => s,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for swap saga {}",
                    saga.id
                )))
            }
        };

        let data = match &saga.data {
            OperationData::Swap(d) => d,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for swap saga {}",
                    saga.id
                )))
            }
        };

        match state {
            SwapSagaState::ProofsReserved => {
                // No external call was made - safe to compensate
                tracing::info!(
                    "Swap saga {} in ProofsReserved state - compensating",
                    saga.id
                );
                self.compensate_swap(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            SwapSagaState::SwapRequested => {
                // External call may have succeeded - check mint
                tracing::info!(
                    "Swap saga {} in SwapRequested state - checking mint for proof states",
                    saga.id
                );
                self.recover_or_compensate_swap(&saga.id, data).await
            }
        }
    }

    /// Check mint and either complete swap or compensate.
    async fn recover_or_compensate_swap(
        &self,
        saga_id: &uuid::Uuid,
        data: &SwapOperationData,
    ) -> Result<RecoveryAction, Error> {
        // Get the reserved proofs for this operation
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            // No proofs found - saga may have already been cleaned up
            tracing::warn!(
                "No reserved proofs found for swap saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        // Check proof states with the mint using the recovery helper
        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
                // Input proofs are spent - swap succeeded, recover outputs
                tracing::info!(
                    "Swap saga {} - input proofs spent, recovering outputs",
                    saga_id
                );
                self.complete_swap_from_restore(saga_id, data, &reserved_proofs)
                    .await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
                // Proofs not spent - swap failed, compensate
                tracing::info!(
                    "Swap saga {} - input proofs not spent, compensating",
                    saga_id
                );
                self.compensate_swap(saga_id).await?;
                Ok(RecoveryAction::Compensated)
            }
            Err(e) => {
                // Can't reach mint - skip for now, retry on next recovery
                tracing::warn!(
                    "Swap saga {} - can't check proof states ({}), skipping",
                    saga_id,
                    e
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Complete a swap by restoring outputs from the mint.
    async fn complete_swap_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &SwapOperationData,
        reserved_proofs: &[crate::types::ProofInfo],
    ) -> Result<(), Error> {
        // Try to restore outputs using stored blinded messages
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Swap",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        // Remove the input proofs (they're spent) and add recovered proofs
        let input_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();

        match new_proofs {
            Some(proofs) => {
                self.localstore.update_proofs(proofs, input_ys).await?;
            }
            None => {
                // Couldn't restore outputs - mark inputs as spent
                tracing::warn!(
                    "Swap saga {} - couldn't restore outputs, marking inputs as spent. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_id
                );
                self.localstore
                    .update_proofs_state(input_ys, State::Spent)
                    .await?;
            }
        }

        // Delete the saga record
        self.localstore.delete_saga(saga_id).await?;

        Ok(())
    }

    /// Compensate a swap saga by releasing reserved proofs.
    async fn compensate_swap(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        let proof_ys = reserved_proofs.iter().map(|p| p.y).collect();

        RevertProofReservation {
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
