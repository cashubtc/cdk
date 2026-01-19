//! Resume logic for send sagas after crash recovery.
//!
//! This module handles resuming incomplete send sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.

use cdk_common::wallet::{OperationData, SendSagaState, WalletSaga};
use tracing::instrument;

use crate::nuts::State;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::{CompensatingAction, RevertProofReservation};
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete send saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **ProofsReserved**: The token hasn't been created yet.
    ///   Safe to compensate by releasing the reserved proofs.
    ///
    /// - **TokenCreated**: The token was created but we don't know if it was redeemed.
    ///   Check the mint to determine if the proofs were spent, then update state accordingly.
    #[instrument(skip(self, saga))]
    pub async fn resume_send_saga(&self, saga: &WalletSaga) -> Result<RecoveryAction, Error> {
        let state = match &saga.state {
            cdk_common::wallet::WalletSagaState::Send(s) => s,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for send saga {}",
                    saga.id
                )))
            }
        };

        let _data = match &saga.data {
            OperationData::Send(d) => d,
            _ => {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for send saga {}",
                    saga.id
                )))
            }
        };

        match state {
            SendSagaState::ProofsReserved => {
                // No token was created - safe to compensate
                tracing::info!(
                    "Send saga {} in ProofsReserved state - compensating",
                    saga.id
                );
                self.compensate_send(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            SendSagaState::TokenCreated => {
                // Token was created but we don't know if it was received
                tracing::info!(
                    "Send saga {} in TokenCreated state - checking proof states",
                    saga.id
                );
                self.recover_or_complete_send(&saga.id).await
            }
            SendSagaState::RollingBack => {
                // We crashed during revocation
                tracing::info!(
                    "Send saga {} in RollingBack state - checking proof states",
                    saga.id
                );
                self.recover_rolling_back_send(&saga.id).await
            }
        }
    }

    /// Recover a saga that crashed during revocation (RollingBack state).
    async fn recover_rolling_back_send(
        &self,
        saga_id: &uuid::Uuid,
    ) -> Result<RecoveryAction, Error> {
        // Get the reserved/pending proofs for this operation
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            // No proofs found - saga may have completed (swap successful)
            tracing::warn!(
                "No reserved proofs found for rolling back send saga {} - assuming swap success",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        // Check proof states with the mint
        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
                // Proofs are spent - swap succeeded (or recipient claimed)
                // In either case, the saga is done.
                tracing::info!(
                    "Send saga {} (RollingBack) - proofs are spent, marking as complete",
                    saga_id
                );
                // Ensure local state matches (Spent)
                let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
                self.localstore
                    .update_proofs_state(proof_ys, State::Spent)
                    .await?;
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
                // Proofs are NOT spent - swap failed or didn't happen.
                // Revert to TokenCreated so user can see it as "Pending" and try again.
                tracing::info!(
                    "Send saga {} (RollingBack) - proofs not spent, reverting to TokenCreated",
                    saga_id
                );

                let current_saga = self
                    .localstore
                    .get_saga(saga_id)
                    .await?
                    .ok_or(Error::Custom("Saga not found during recovery".to_string()))?;

                let mut revert_saga = current_saga;
                revert_saga.update_state(cdk_common::wallet::WalletSagaState::Send(
                    cdk_common::wallet::SendSagaState::TokenCreated,
                ));

                self.localstore.update_saga(revert_saga).await?;

                let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
                self.localstore
                    .update_proofs_state(proof_ys, State::PendingSpent)
                    .await?;

                Ok(RecoveryAction::Recovered)
            }
            Err(e) => {
                tracing::warn!(
                    "Send saga {} (RollingBack) - can't check proof states ({}), skipping",
                    saga_id,
                    e
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Check mint and update send saga state accordingly.
    async fn recover_or_complete_send(
        &self,
        saga_id: &uuid::Uuid,
    ) -> Result<RecoveryAction, Error> {
        // Get the reserved/pending proofs for this operation
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            // No proofs found - saga may have completed
            tracing::warn!(
                "No reserved proofs found for send saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();

        // Check proof states with the mint
        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
                // Token was redeemed - mark proofs as spent and clean up
                tracing::info!(
                    "Send saga {} - proofs are spent, marking as complete",
                    saga_id
                );
                self.localstore
                    .update_proofs_state(proof_ys, State::Spent)
                    .await?;
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
                // Token wasn't redeemed - leave proofs in PendingSpent state
                // The user still has the token and could redeem it later
                tracing::info!(
                    "Send saga {} - proofs not spent, token may still be valid",
                    saga_id
                );
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            Err(e) => {
                tracing::warn!(
                    "Send saga {} - can't check proof states ({}), skipping",
                    saga_id,
                    e
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Compensate a send saga by releasing reserved proofs.
    async fn compensate_send(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
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
    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::wallet::{
        OperationData, SendOperationData, SendSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::test_utils::*;

    #[tokio::test]
    async fn test_recover_send_proofs_reserved() {
        // Test that send saga in ProofsReserved state gets compensated
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and store proofs, then reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in ProofsReserved state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::ProofsReserved),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: None,
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.compensated, 1);

        // Verify proofs are released
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Verify saga is deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }
}
