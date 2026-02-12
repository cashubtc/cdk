//! Resume logic for receive sagas after crash recovery.
//!
//! Handles incomplete receive sagas interrupted by a crash.
//! Determines the actual state by querying the mint and either completes
//! the operation or compensates.
//!
//! # Recovery Strategy
//!
//! For `SwapRequested` state, uses a replay-first strategy:
//! - **Replay**: Attempt to replay the original `post_swap` request.
//!   If the mint cached the response (NUT-19), signatures are returned immediately.
//! - **Fallback**: If replay fails, check if inputs are spent and use `/restore`.

use cdk_common::wallet::{OperationData, ReceiveOperationData, ReceiveSagaState, WalletSaga};
use tracing::instrument;

use crate::wallet::receive::saga::compensation::RemovePendingProofs;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete receive saga after crash recovery.
    ///
    /// For `ProofsPending` state, compensates by removing pending proofs.
    /// For `SwapRequested` state, checks if input proofs are spent and either
    /// recovers outputs or compensates.
    #[instrument(skip(self, saga))]
    pub(crate) async fn resume_receive_saga(
        &self,
        saga: &WalletSaga,
    ) -> Result<RecoveryAction, Error> {
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
    /// Uses a replay-first strategy: first attempts to replay the original swap
    /// request (leverages NUT-19 caching). If replay fails, falls back to
    /// checking proof states and using /restore.
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
    use std::sync::Arc;

    use cdk_common::nuts::{CheckStateResponse, CurrencyUnit, ProofState, RestoreResponse, State};
    use cdk_common::wallet::{
        OperationData, ReceiveOperationData, ReceiveSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::recovery::RecoveryAction;
    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{create_test_wallet_with_mock, MockMintConnector};

    #[tokio::test]
    async fn test_recover_receive_proofs_pending() {
        // Compensate: remove pending proofs
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create proofs in Unspent state and reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in ProofsPending state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Receive(ReceiveSagaState::ProofsPending),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                token: Some("test_token".to_string()),
                counter_start: None,
                counter_end: None,
                amount: Some(Amount::from(100)),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Create wallet and recover
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_receive_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Verify compensation
        assert!(result.is_ok());
        let recovery_action = result.unwrap();
        assert_eq!(recovery_action, RecoveryAction::Compensated);

        // Pending proofs should be removed
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_receive_swap_requested_replay_succeeds() {
        // Mock: post_swap succeeds → recovered
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create proofs in Unspent state and reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in SwapRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Receive(ReceiveSagaState::SwapRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                token: Some("test_token".to_string()),
                counter_start: Some(0),
                counter_end: Some(10),
                amount: Some(Amount::from(100)),
                blinded_messages: Some(vec![]), // Empty for simplicity
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: check_state returns Unspent (swap hasn't happened yet)
        // and post_swap succeeds
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Unspent, // Not spent yet
                witness: None,
            }],
        }));
        mock_client.set_post_swap_response(Ok(crate::nuts::SwapResponse { signatures: vec![] }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_receive_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Verify recovery
        assert!(result.is_ok());
        let recovery_action = result.unwrap();
        assert_eq!(recovery_action, RecoveryAction::Compensated);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // Proof should be removed (compensation deletes pending proofs)
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        // No transactions should be recorded
        assert!(db
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_recover_receive_swap_requested_proofs_spent() {
        // Mock: check_state returns Spent, restore succeeds → recovered
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create proofs in Unspent state and reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create saga in SwapRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Receive(ReceiveSagaState::SwapRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                token: Some("test_token".to_string()),
                counter_start: Some(0),
                counter_end: Some(10),
                amount: Some(Amount::from(100)),
                blinded_messages: Some(vec![]),
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: check_state returns Spent (swap happened at mint)
        // and restore returns new proofs
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Spent, // Spent at mint
                witness: None,
            }],
        }));
        mock_client._set_restore_response(Ok(RestoreResponse {
            signatures: vec![],
            outputs: vec![],
            promises: None,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_receive_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Should recover via restore
        assert!(result.is_ok());
        let recovery_action = result.unwrap();
        assert_eq!(recovery_action, RecoveryAction::Recovered);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // Proof marked spent/removed
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());
        let reserved = db.get_reserved_proofs(&saga_id).await.unwrap();
        assert!(reserved.is_empty());

        // Note: Receive saga does not record transactions
    }
}
