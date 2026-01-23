//! Resume logic for swap sagas after crash recovery.
//!
//! This module handles resuming incomplete swap sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.
//!
//! # Recovery Strategy
//!
//! For `SwapRequested` state, we use a replay-first strategy:
//! 1. **Replay**: Attempt to replay the original `post_swap` request.
//!    If the mint cached the response (NUT-19), we get signatures immediately.
//! 2. **Fallback**: If replay fails, check if inputs are spent and use `/restore`.

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
                tracing::info!(
                    "Swap saga {} in ProofsReserved state - compensating",
                    saga.id
                );
                self.compensate_swap(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            SwapSagaState::SwapRequested => {
                tracing::info!(
                    "Swap saga {} in SwapRequested state - checking mint for proof states",
                    saga.id
                );
                self.recover_or_compensate_swap(&saga.id, data).await
            }
        }
    }

    /// Check mint and either complete swap or compensate.
    ///
    /// Uses a replay-first strategy:
    /// 1. Try to replay the original swap request (leverages NUT-19 caching)
    /// 2. If replay fails, fall back to checking proof states and /restore
    async fn recover_or_compensate_swap(
        &self,
        saga_id: &uuid::Uuid,
        data: &SwapOperationData,
    ) -> Result<RecoveryAction, Error> {
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            tracing::warn!(
                "No reserved proofs found for swap saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        if let Some(new_proofs) = self
            .try_replay_swap_request(
                saga_id,
                "Swap",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
                &reserved_proofs,
            )
            .await?
        {
            let input_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
            self.localstore.update_proofs(new_proofs, input_ys).await?;
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
                tracing::info!(
                    "Swap saga {} - input proofs spent, recovering outputs via /restore",
                    saga_id
                );
                self.complete_swap_from_restore(saga_id, data, &reserved_proofs)
                    .await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
                tracing::info!(
                    "Swap saga {} - input proofs not spent, compensating",
                    saga_id
                );
                self.compensate_swap(saga_id).await?;
                Ok(RecoveryAction::Compensated)
            }
            Err(e) => {
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
        reserved_proofs: &[cdk_common::wallet::ProofInfo],
    ) -> Result<(), Error> {
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Swap",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        let input_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();

        match new_proofs {
            Some(proofs) => {
                self.localstore.update_proofs(proofs, input_ys).await?;
            }
            None => {
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
    use std::sync::Arc;

    use cdk_common::nuts::{CheckStateResponse, CurrencyUnit, ProofState, State};
    use cdk_common::wallet::{
        OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::test_utils::*;

    #[tokio::test]
    async fn test_recover_swap_proofs_reserved() {
        // Test that swap saga in ProofsReserved state gets compensated:
        // - Reserved proofs are released back to Unspent
        // - Saga record is deleted
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
            WalletSagaState::Swap(SwapSagaState::ProofsReserved),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Swap(SwapOperationData {
                input_amount: Amount::from(100),
                output_amount: Amount::from(90),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Create wallet and run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Verify compensation occurred
        assert_eq!(report.compensated, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.failed, 0);

        // Verify proofs are released (back to Unspent)
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].y, proof_y);

        // Verify saga is deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_swap_requested_proofs_not_spent() {
        // When proofs are NOT spent, the swap failed - should compensate
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in SwapRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Swap(SwapSagaState::SwapRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Swap(SwapOperationData {
                input_amount: Amount::from(100),
                output_amount: Amount::from(90),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: proofs are NOT spent (swap failed)
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Unspent, // NOT spent - swap failed
                witness: None,
            }],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Should compensate (proofs not spent means swap failed)
        assert_eq!(report.compensated, 1);
        assert_eq!(report.recovered, 0);

        // Proofs should be released back to Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_swap_requested_mint_unreachable() {
        // When mint is unreachable, should skip (retry later)
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in SwapRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Swap(SwapSagaState::SwapRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Swap(SwapOperationData {
                input_amount: Amount::from(100),
                output_amount: Amount::from(90),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: mint is unreachable
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client
            .set_check_state_response(Err(crate::Error::Custom("Connection refused".to_string())));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Should skip (mint unreachable, retry later)
        assert_eq!(report.skipped, 1);
        assert_eq!(report.compensated, 0);
        assert_eq!(report.recovered, 0);

        // Proofs should still be reserved
        let reserved = db.get_reserved_proofs(&saga_id).await.unwrap();
        assert_eq!(reserved.len(), 1);

        // Saga should still exist
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
    }
}
