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
    #[instrument(skip(self, saga))]
    pub(crate) async fn resume_send_saga(
        &self,
        saga: &WalletSaga,
    ) -> Result<RecoveryAction, Error> {
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
                tracing::info!(
                    "Send saga {} in ProofsReserved state - compensating",
                    saga.id
                );
                self.compensate_send(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            SendSagaState::TokenCreated => {
                tracing::info!(
                    "Send saga {} in TokenCreated state - checking proof states",
                    saga.id
                );
                self.recover_or_complete_send(&saga.id).await
            }
            SendSagaState::RollingBack => {
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
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            tracing::warn!(
                "No reserved proofs found for rolling back send saga {} - assuming swap success",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
                tracing::info!(
                    "Send saga {} (RollingBack) - proofs are spent, marking as complete",
                    saga_id
                );
                let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();
                self.localstore
                    .update_proofs_state(proof_ys, State::Spent)
                    .await?;
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            Ok(false) => {
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
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if reserved_proofs.is_empty() {
            tracing::warn!(
                "No reserved proofs found for send saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        let proof_ys: Vec<_> = reserved_proofs.iter().map(|p| p.y).collect();

        match self.are_proofs_spent(&reserved_proofs).await {
            Ok(true) => {
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
                tracing::info!(
                    "Send saga {} - proofs not spent, token may still be valid",
                    saga_id
                );
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
    use std::sync::Arc;

    use cdk_common::nuts::{CheckStateResponse, CurrencyUnit, ProofState, State};
    use cdk_common::wallet::{
        OperationData, SendOperationData, SendSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{
        create_test_wallet, create_test_wallet_with_mock, MockMintConnector,
    };

    #[tokio::test]
    async fn test_recover_send_proofs_reserved() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and store proofs, then reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

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

        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.compensated, 1);

        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_send_token_created_proofs_spent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::PendingSpent)
            .await
            .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::TokenCreated),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Spent,
                witness: None,
            }],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.recovered, 1);
        assert_eq!(report.compensated, 0);

        let proofs = db
            .get_proofs(None, None, Some(vec![State::Spent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_send_token_created_proofs_not_spent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::PendingSpent)
            .await
            .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::TokenCreated),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Unspent,
                witness: None,
            }],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.recovered, 1);
        assert_eq!(report.compensated, 0);

        let proofs = db
            .get_proofs(None, None, Some(vec![State::PendingSpent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_recover_send_rolling_back_proofs_spent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::PendingSpent)
            .await
            .unwrap();

        // Create saga in RollingBack state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::RollingBack),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: proofs are spent
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Spent,
                witness: None,
            }],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.recovered, 1);

        let proofs = db
            .get_proofs(None, None, Some(vec![State::Spent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_send_rolling_back_proofs_not_spent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();
        db.update_proofs_state(vec![proof_y], State::PendingSpent)
            .await
            .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::RollingBack),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: proof_y,
                state: State::Unspent,
                witness: None,
            }],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.recovered, 1);

        let proofs = db
            .get_proofs(None, None, Some(vec![State::PendingSpent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        let saga = db.get_saga(&saga_id).await.unwrap().unwrap();
        assert!(matches!(
            saga.state,
            WalletSagaState::Send(SendSagaState::TokenCreated)
        ));
    }
}
