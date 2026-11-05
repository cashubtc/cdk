//! Resume logic for send sagas after crash recovery.
//!
//! This module handles resuming incomplete send sagas that were interrupted
//! by a crash. It determines the actual state by querying the mint and
//! either completes the operation or compensates.

use cdk_common::wallet::{OperationData, ProofInfo, SendSagaState, TransactionStatus, WalletSaga};
use tracing::instrument;

use crate::nuts::{CheckStateRequest, Proof, State};
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::{CompensatingAction, RevertProofReservation};
use crate::wallet::util::escape_log_value;
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

        let data = match &saga.data {
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
                if data.counter_start.is_some() {
                    let proofs = self.localstore.get_reserved_proofs(&saga.id).await?;
                    if !proofs.is_empty() {
                        let ys: std::collections::HashSet<_> =
                            proofs.iter().map(|proof| proof.y).collect();
                        let response = match self
                            .client
                            .post_check_state(CheckStateRequest {
                                ys: ys.iter().copied().collect(),
                            })
                            .await
                        {
                            Ok(response) => response,
                            Err(_) => return Ok(RecoveryAction::Skipped),
                        };
                        let returned: std::collections::HashSet<_> =
                            response.states.iter().map(|state| state.y).collect();
                        if response.states.len() != ys.len()
                            || returned != ys
                            || response
                                .states
                                .iter()
                                .any(|state| state.state == State::Pending)
                        {
                            return Ok(RecoveryAction::Skipped);
                        }
                        let spent = response
                            .states
                            .into_iter()
                            .filter(|state| state.state == State::Spent)
                            .map(|state| state.y)
                            .collect();
                        self.localstore
                            .update_proofs_state(spent, State::Spent)
                            .await?;
                    }
                }
                self.mark_transaction_failed(saga.id).await?;
                self.compensate_send(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            SendSagaState::TokenCreated => {
                tracing::info!(
                    "Send saga {} in TokenCreated state - checking proof states",
                    saga.id
                );
                self.recover_or_complete_send(&saga.id, data.proofs.as_deref())
                    .await
            }
            SendSagaState::RollingBack => {
                tracing::info!(
                    "Send saga {} in RollingBack state - checking proof states",
                    saga.id
                );
                self.recover_rolling_back_send(&saga.id, data.proofs.as_deref())
                    .await
            }
        }
    }

    /// Recover a saga that crashed during revocation (RollingBack state).
    async fn recover_rolling_back_send(
        &self,
        saga_id: &uuid::Uuid,
        sent_proofs: Option<&[Proof]>,
    ) -> Result<RecoveryAction, Error> {
        let reserved_proofs = self.send_recovery_proofs(saga_id, sent_proofs).await?;

        if reserved_proofs.is_empty() {
            tracing::warn!(
                "No reserved proofs found for rolling back send saga {} - assuming swap success",
                saga_id
            );
            self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Failed)
                .await?;
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
                self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Failed)
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
                    escape_log_value(&e)
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Check mint and update send saga state accordingly.
    async fn recover_or_complete_send(
        &self,
        saga_id: &uuid::Uuid,
        sent_proofs: Option<&[Proof]>,
    ) -> Result<RecoveryAction, Error> {
        let reserved_proofs = self.send_recovery_proofs(saga_id, sent_proofs).await?;

        if reserved_proofs.is_empty() {
            tracing::warn!(
                "No reserved proofs found for send saga {} - cleaning up orphaned saga",
                saga_id
            );
            self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Completed)
                .await?;
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
                self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Completed)
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
                    escape_log_value(&e)
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    // Original reservations can be spent swap inputs. Both confirmation and
    // revocation recovery must follow the token's persisted output proofs.
    async fn send_recovery_proofs(
        &self,
        saga_id: &uuid::Uuid,
        sent_proofs: Option<&[Proof]>,
    ) -> Result<Vec<ProofInfo>, Error> {
        match sent_proofs {
            Some(proofs) => proofs
                .iter()
                .map(|proof| {
                    ProofInfo::new(
                        proof.clone(),
                        self.mint_url.clone(),
                        State::PendingSpent,
                        self.unit.clone(),
                    )
                })
                .collect::<Result<Vec<_>, _>>(),
            None => self
                .localstore
                .get_reserved_proofs(saga_id)
                .await
                .map_err(Into::into),
        }
    }

    /// Compensate a send saga by releasing reserved proofs.
    async fn compensate_send(&self, saga_id: &uuid::Uuid) -> Result<(), Error> {
        let reserved_proofs = self.localstore.get_reserved_proofs(saga_id).await?;
        let proof_ys = reserved_proofs.iter().map(|p| p.y).collect();

        let mut pending_spent_proofs: Vec<_> = reserved_proofs
            .into_iter()
            .filter(|p| p.state == State::PendingSpent)
            .collect();

        for proof in pending_spent_proofs.iter_mut() {
            proof.state = State::Reserved;
        }

        if !pending_spent_proofs.is_empty() {
            tracing::warn!(
                "Send saga {} in ProofsReserved state has {} PendingSpent proofs; reverting them",
                saga_id,
                pending_spent_proofs.len()
            );
            self.localstore
                .update_proofs(pending_spent_proofs, vec![])
                .await?;
        }

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
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk_common::nuts::{CheckStateResponse, CurrencyUnit, ProofState, State};
    use cdk_common::wallet::{
        OperationData, SendOperationData, SendSagaState, Transaction, TransactionDirection,
        TransactionStatus, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{
        create_test_wallet, create_test_wallet_with_mock, MockMintConnector,
    };

    #[tokio::test]
    async fn checkpointed_send_waits_for_complete_terminal_input_states() {
        for response_kind in 0..5 {
            let db = create_test_db().await;
            let saga_id = uuid::Uuid::new_v4();
            let proof = test_proof_info(test_keyset_id(), 100, test_mint_url(), State::Unspent);
            let y = proof.y;
            db.update_proofs(vec![proof], vec![]).await.unwrap();
            db.reserve_proofs(vec![y], &saga_id).await.unwrap();
            db.add_saga(WalletSaga::new(
                saga_id,
                WalletSagaState::Send(SendSagaState::ProofsReserved),
                100.into(),
                test_mint_url(),
                CurrencyUnit::Sat,
                OperationData::Send(SendOperationData {
                    amount: 100.into(),
                    output_plan: None,
                    memo: None,
                    counter_start: Some(0),
                    counter_end: None,
                    token: None,
                    proofs: None,
                }),
            ))
            .await
            .unwrap();
            let client = Arc::new(MockMintConnector::new());
            let state = ProofState {
                y,
                state: State::Unspent,
                witness: None,
            };
            let response = match response_kind {
                0 => Err(crate::Error::Timeout),
                1 => Ok(CheckStateResponse { states: vec![] }),
                2 => Ok(CheckStateResponse {
                    states: vec![ProofState {
                        state: State::Pending,
                        ..state
                    }],
                }),
                3 => Ok(CheckStateResponse {
                    states: vec![state.clone(), state],
                }),
                _ => Ok(CheckStateResponse {
                    states: vec![ProofState {
                        y: crate::nuts::SecretKey::generate().public_key(),
                        ..state
                    }],
                }),
            };
            client.set_check_state_response(response);
            let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;
            let report = wallet.recover_incomplete_sagas().await.unwrap();
            assert_eq!(report.skipped, 1);
            let stored = db.get_proofs_by_ys(vec![y]).await.unwrap();
            assert_eq!(stored[0].state, State::Reserved);
            assert_eq!(stored[0].used_by_operation, Some(saga_id));
            assert!(db.get_saga(&saga_id).await.unwrap().is_some());
            client.set_check_state_response(Ok(CheckStateResponse {
                states: vec![ProofState {
                    y,
                    state: State::Spent,
                    witness: None,
                }],
            }));
            let report = wallet.recover_incomplete_sagas().await.unwrap();
            assert_eq!(report.compensated, 1);
            let stored = db.get_proofs_by_ys(vec![y]).await.unwrap();
            assert_eq!(stored[0].state, State::Spent);
            assert!(db.get_saga(&saga_id).await.unwrap().is_none());
        }
    }

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
                output_plan: None,
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
        assert_eq!(proofs[0].used_by_operation, None);

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_send_proofs_reserved_with_pending_spent_proofs_reverts_them() {
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
            WalletSagaState::Send(SendSagaState::ProofsReserved),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                output_plan: None,
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

        let proofs = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].state, State::Unspent);
        assert_eq!(proofs[0].used_by_operation, None);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_send_proofs_reserved_without_operation_link_leaves_reserved_proof() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        db.update_proofs_state(vec![proof_y], State::Reserved)
            .await
            .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Send(SendSagaState::ProofsReserved),
            Amount::from(100),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount: Amount::from(100),
                output_plan: None,
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

        assert_eq!(report.recovered, 0);
        assert_eq!(report.compensated, 1);

        let reserved = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].state, State::Reserved);
        assert_eq!(reserved[0].used_by_operation, None);

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
                output_plan: None,
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();
        db.add_transaction(Transaction {
            mint_url: mint_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(100),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![proof_y],
            timestamp: 42,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status: TransactionStatus::Pending,
        })
        .await
        .unwrap();

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
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions[0].status, TransactionStatus::Completed);
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
                output_plan: None,
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
                output_plan: None,
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some("cashuA...".to_string()),
                proofs: None,
            }),
        );
        db.add_saga(saga).await.unwrap();
        db.add_transaction(Transaction {
            mint_url: mint_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(100),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![proof_y],
            timestamp: 42,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status: TransactionStatus::Pending,
        })
        .await
        .unwrap();

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
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions[0].status, TransactionStatus::Failed);
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
                output_plan: None,
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
