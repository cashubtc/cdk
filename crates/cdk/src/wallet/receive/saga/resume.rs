//! Resume logic for receive sagas after crash recovery.
//!
//! This module handles resuming incomplete receive sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.
//!
//! # Recovery Strategy
//!
//! For `SwapRequested` state, we use a replay-first strategy:
//! 1. **Replay**: Attempt to replay the original `post_swap` request.
//!    If the mint cached the response (NUT-19), we get signatures immediately.
//! 2. **Fallback**: If replay fails, check if inputs are spent and use `/restore`.

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
                tracing::info!(
                    "Receive saga {} in ProofsPending state - compensating",
                    saga.id
                );
                self.compensate_receive(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            ReceiveSagaState::SwapRequested => {
                tracing::info!(
                    "Receive saga {} in SwapRequested state - checking mint for proof states",
                    saga.id
                );
                self.recover_or_compensate_receive(&saga.id, data).await
            }
        }
    }

    /// Check mint and either complete receive or compensate.
    ///
    /// Uses a replay-first strategy:
    /// 1. Try to replay the original swap request (leverages NUT-19 caching)
    /// 2. If replay fails, fall back to checking proof states and /restore
    async fn recover_or_compensate_receive(
        &self,
        saga_id: &uuid::Uuid,
        data: &ReceiveOperationData,
    ) -> Result<RecoveryAction, Error> {
        let pending_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if pending_proofs.is_empty() {
            tracing::warn!(
                "No pending proofs found for receive saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        if let Some(new_proofs) = self
            .try_replay_swap_request(
                saga_id,
                "Receive",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
                &pending_proofs,
            )
            .await?
        {
            let input_ys: Vec<_> = pending_proofs.iter().map(|p| p.y).collect();
            self.localstore.update_proofs(new_proofs, input_ys).await?;
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        match self.are_proofs_spent(&pending_proofs).await {
            Ok(true) => {
                tracing::info!(
                    "Receive saga {} - input proofs spent, recovering outputs via /restore",
                    saga_id
                );
                self.complete_receive_from_restore(saga_id, data, &pending_proofs)
                    .await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
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
        pending_proofs: &[cdk_common::wallet::ProofInfo],
    ) -> Result<(), Error> {
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Receive",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        let input_ys: Vec<_> = pending_proofs.iter().map(|p| p.y).collect();

        match new_proofs {
            Some(proofs) => {
                self.localstore.update_proofs(proofs, input_ys).await?;
            }
            None => {
                tracing::warn!(
                    "Receive saga {} - couldn't restore outputs, removing spent inputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_id
                );
                self.localstore.update_proofs(vec![], input_ys).await?;
            }
        }

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
