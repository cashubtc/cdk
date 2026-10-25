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
//! - **Fallback**: If replay fails, restore and validate the expected outputs.

use std::collections::HashMap;

use cdk_common::wallet::{
    OperationData, ReceiveOperationData, ReceiveSagaState, Transaction, TransactionDirection,
    TransactionId, TransactionStatus, WalletSaga,
};
use cdk_common::{Amount, ProofsMethods};
use tracing::instrument;

use crate::nuts::{CheckStateRequest, PreMintSecrets, PublicKey, State};
use crate::util::unix_time;
use crate::wallet::receive::saga::compensation::RemovePendingProofs;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::wallet::util::escape_log_value;
use crate::{Error, Wallet};

impl Wallet {
    /// Resume an incomplete receive saga after crash recovery.
    ///
    /// For `ProofsPending` state, compensates by removing pending proofs.
    /// For `SwapRequested` state, recovers validated outputs or retains the saga
    /// for retry.
    #[instrument(skip(self, saga))]
    pub(crate) async fn resume_receive_saga(
        &self,
        saga: &WalletSaga,
    ) -> Result<RecoveryAction, Error> {
        let saga = match self.localstore.get_saga(&saga.id).await? {
            Some(saga) => saga,
            None => return Ok(RecoveryAction::Skipped),
        };
        if let Some(transaction) = self
            .localstore
            .get_transaction(TransactionId::from_saga_id(saga.id))
            .await?
        {
            match transaction.status {
                TransactionStatus::Failed => {
                    self.compensate_receive(&saga.id).await?;
                    return Ok(RecoveryAction::Compensated);
                }
                TransactionStatus::Pending | TransactionStatus::Completed => (),
            }
        }
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
                if self
                    .localstore
                    .get_transaction(TransactionId::from_saga_id(saga.id))
                    .await?
                    .is_some_and(|t| t.status == TransactionStatus::Completed)
                {
                    return Ok(RecoveryAction::Skipped);
                }
                tracing::info!(
                    "Receive saga {} in ProofsPending state - compensating",
                    saga.id
                );
                self.mark_transaction_failed(saga.id).await?;
                self.compensate_receive(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            ReceiveSagaState::SwapRequested => {
                tracing::info!(
                    "Receive saga {} in SwapRequested state - recovering expected outputs",
                    saga.id
                );
                self.recover_or_compensate_receive(&saga, data).await
            }
        }
    }

    /// Check the mint and complete only with validated outputs.
    ///
    /// Uses a replay-first strategy: first attempts to replay the original swap
    /// request (leverages NUT-19 caching). If replay fails, restores and
    /// validates the expected outputs.
    async fn recover_or_compensate_receive(
        &self,
        saga: &WalletSaga,
        data: &ReceiveOperationData,
    ) -> Result<RecoveryAction, Error> {
        let saga_id = &saga.id;
        let pending_proofs = self.localstore.get_reserved_proofs(saga_id).await?;

        if pending_proofs.is_empty() {
            return self.complete_receive_from_local_outputs(saga, data).await;
        }

        let proof_ys: Vec<_> = pending_proofs.iter().map(|proof| proof.y).collect();
        let transaction_id = TransactionId::from_saga_id(*saga_id);
        if self
            .localstore
            .get_transaction(transaction_id)
            .await?
            .is_none()
        {
            let proofs = pending_proofs
                .iter()
                .map(|proof| proof.proof.clone())
                .collect::<Vec<_>>();
            let input_amount = data.amount.unwrap_or(proofs.total_amount()?);
            let (amount, fee) = match data.blinded_messages.as_deref() {
                Some(outputs) if !outputs.is_empty() => {
                    let amount = Amount::try_sum(outputs.iter().map(|output| output.amount))?;
                    let fee = input_amount.checked_sub(amount).ok_or_else(|| {
                        Error::InvalidMintResponse(
                            "receive outputs exceed the input value".to_owned(),
                        )
                    })?;
                    (amount, fee)
                }
                _ => {
                    let fee = match self.get_proofs_fee(&proofs).await {
                        Ok(fee) => fee.total,
                        Err(error) => {
                            tracing::warn!(
                                "Receive saga {} - couldn't reconstruct the historical fee ({}), \
                                 recording the pending transaction with zero fee",
                                saga_id,
                                escape_log_value(&error)
                            );
                            Amount::ZERO
                        }
                    };
                    let amount = data
                        .amount
                        .unwrap_or(input_amount)
                        .checked_sub(fee)
                        .unwrap_or(Amount::ZERO);
                    (amount, fee)
                }
            };

            self.upsert_transaction(Transaction {
                mint_url: self.mint_url.clone(),
                direction: TransactionDirection::Incoming,
                amount,
                fee,
                unit: self.unit.clone(),
                ys: proof_ys,
                timestamp: unix_time(),
                memo: None,
                metadata: HashMap::new(),
                quote_id: None,
                payment_request: None,
                payment_proof: None,
                payment_method: None,
                saga_id: Some(*saga_id),
                status: TransactionStatus::Pending,
            })
            .await?;
        }

        if let Some(action) = self
            .try_complete_receive_from_local_outputs(saga, data)
            .await?
        {
            return Ok(action);
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
            return self.finalize_receive_from_mint(saga, new_proofs).await;
        }

        self.complete_receive_from_restore(saga, data).await
    }

    /// Complete a receive by restoring outputs from the mint.
    async fn complete_receive_from_restore(
        &self,
        saga: &WalletSaga,
        data: &ReceiveOperationData,
    ) -> Result<RecoveryAction, Error> {
        let new_proofs = self
            .restore_outputs(
                &saga.id,
                "Receive",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        match new_proofs {
            Some(proofs) => self.finalize_receive_from_mint(saga, proofs).await,
            None => Ok(RecoveryAction::Skipped),
        }
    }

    async fn finalize_recovered_receive(
        &self,
        saga: &WalletSaga,
        proofs: Vec<cdk_common::wallet::ProofInfo>,
        persisted_ys: Vec<PublicKey>,
    ) -> Result<RecoveryAction, Error> {
        let already_completed = self
            .localstore
            .get_transaction(TransactionId::from_saga_id(saga.id))
            .await?
            .is_some_and(|t| t.status == TransactionStatus::Completed);
        match self
            .localstore
            .finalize_receive(&saga.id, saga.version, proofs, persisted_ys)
            .await?
        {
            true if already_completed => Ok(RecoveryAction::Skipped),
            true => Ok(RecoveryAction::Recovered),
            false => Ok(RecoveryAction::Skipped),
        }
    }

    async fn finalize_receive_from_mint(
        &self,
        saga: &WalletSaga,
        mut proofs: Vec<cdk_common::wallet::ProofInfo>,
    ) -> Result<RecoveryAction, Error> {
        if self.localstore.get_saga(&saga.id).await?.is_none() {
            return Ok(RecoveryAction::Skipped);
        }
        let ys = proofs.iter().map(|p| p.y).collect();
        let persisted = self.localstore.get_proofs_by_ys(ys).await?;
        let persisted_ys: Vec<_> = persisted.iter().map(|p| p.y).collect();
        if !persisted.iter().all(|stored| {
            proofs.iter().any(|restored| {
                restored.y == stored.y
                    && restored.proof.amount == stored.proof.amount
                    && restored.proof.keyset_id == stored.proof.keyset_id
                    && restored.mint_url == stored.mint_url
                    && restored.unit == stored.unit
            })
        }) {
            return Err(Error::InvalidMintResponse(
                "persisted receive output differs from restored output".to_owned(),
            ));
        }
        let missing: Vec<_> = proofs
            .iter()
            .filter(|p| !persisted_ys.contains(&p.y))
            .map(|p| p.y)
            .collect();
        let atomic_settlement = matches!(
            &saga.data,
            OperationData::Receive(data) if data.atomic_settlement
        );
        let inputs_retained = !self
            .localstore
            .get_reserved_proofs(&saga.id)
            .await?
            .is_empty();
        let pending_receipt = self
            .localstore
            .get_transaction(TransactionId::from_saga_id(saga.id))
            .await?
            .is_some_and(|t| t.status == TransactionStatus::Pending);
        if !missing.is_empty() && !(atomic_settlement && inputs_retained && pending_receipt) {
            let response = self
                .client
                .post_check_state(CheckStateRequest {
                    ys: missing.clone(),
                })
                .await?;
            let mut states = HashMap::new();
            for proof_state in response.states {
                if !missing.contains(&proof_state.y)
                    || !matches!(
                        proof_state.state,
                        State::Unspent | State::Pending | State::Spent
                    )
                    || states.insert(proof_state.y, proof_state.state).is_some()
                {
                    return Err(Error::InvalidMintResponse(
                        "invalid receive output state response".to_owned(),
                    ));
                }
            }
            if states.len() != missing.len() {
                return Err(Error::InvalidMintResponse(
                    "incomplete receive output state response".to_owned(),
                ));
            }
            for proof in &mut proofs {
                if let Some(state) = states.get(&proof.y) {
                    proof.state = *state;
                }
            }
        }
        self.finalize_recovered_receive(saga, proofs, persisted_ys)
            .await
    }

    /// Recognize a legacy interruption after the atomic proof replacement.
    /// Keep existing spent and reserved proof states intact.
    async fn complete_receive_from_local_outputs(
        &self,
        saga: &WalletSaga,
        data: &ReceiveOperationData,
    ) -> Result<RecoveryAction, Error> {
        match self
            .try_complete_receive_from_local_outputs(saga, data)
            .await?
        {
            Some(action) => Ok(action),
            None => self.complete_receive_from_restore(saga, data).await,
        }
    }

    async fn try_complete_receive_from_local_outputs(
        &self,
        saga: &WalletSaga,
        data: &ReceiveOperationData,
    ) -> Result<Option<RecoveryAction>, Error> {
        let (outputs, start, end) =
            match (&data.blinded_messages, data.counter_start, data.counter_end) {
                (Some(outputs), Some(start), Some(end)) if !outputs.is_empty() => {
                    (outputs, start, end)
                }
                _ => return Ok(None),
            };
        let premints = PreMintSecrets::restore_batch(outputs[0].keyset_id, &self.seed, start, end)?;
        let mut expected = Vec::with_capacity(outputs.len());
        for output in outputs {
            let premint = match premints
                .secrets
                .iter()
                .find(|p| p.blinded_message.blinded_secret == output.blinded_secret)
            {
                Some(premint) => premint,
                None => return Ok(None),
            };
            let y = crate::dhke::hash_to_curve(&premint.secret.to_bytes())?;
            if expected.contains(&y) {
                return Ok(None);
            }
            expected.push(y);
        }
        let proofs = self.localstore.get_proofs_by_ys(expected.clone()).await?;
        if proofs.len() != outputs.len()
            || !outputs.iter().zip(expected).all(|(output, y)| {
                proofs.iter().any(|p| {
                    p.y == y
                        && p.proof.keyset_id == output.keyset_id
                        && p.proof.amount == output.amount
                        && p.mint_url == self.mint_url
                        && p.unit == self.unit
                })
            })
        {
            return Ok(None);
        }
        let persisted_ys = proofs.iter().map(|p| p.y).collect();
        self.finalize_recovered_receive(saga, proofs, persisted_ys)
            .await
            .map(Some)
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
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;
    use cdk_common::nuts::{CheckStateResponse, CurrencyUnit, ProofState, RestoreResponse, State};
    use cdk_common::wallet::{
        OperationData, ReceiveOperationData, ReceiveSagaState, Transaction, TransactionDirection,
        TransactionStatus, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::recovery::RecoveryAction;
    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{create_test_wallet_with_mock, MockMintConnector};

    async fn interrupted_receive(
        db: Arc<
            dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync,
        >,
    ) -> (
        crate::Wallet,
        Arc<MockMintConnector>,
        WalletSaga,
        RestoreResponse,
    ) {
        interrupted_receive_with_options(db, crate::wallet::ReceiveOptions::default()).await
    }

    async fn interrupted_receive_with_options(
        db: Arc<
            dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync,
        >,
        options: crate::wallet::ReceiveOptions,
    ) -> (
        crate::Wallet,
        Arc<MockMintConnector>,
        WalletSaga,
        RestoreResponse,
    ) {
        use crate::nuts::{BlindSignature, Token};

        let client = Arc::new(MockMintConnector::new());
        client.enable_mint_signing();
        *client.recovered_output_state.lock().unwrap() = Some(State::Unspent);
        let keyset_id = client.keysets.lock().unwrap()[0].id;
        let mut input = crate::wallet::test_utils::test_proof(keyset_id, 8);
        input.c = crate::dhke::sign_message(
            &client.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&input.amount],
            &input.y().unwrap(),
        )
        .unwrap();
        let token = Token::new(test_mint_url(), vec![input], None, CurrencyUnit::Sat);
        let wallet = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            client.clone(),
            [42; 64],
        )
        .await;
        client.set_post_swap_response(Err(crate::Error::Timeout));
        assert!(matches!(
            wallet.receive(&token.to_string(), options).await,
            Err(crate::Error::Timeout)
        ));
        let saga = db.get_incomplete_sagas().await.unwrap().pop().unwrap();
        let request = client.captured_swap_requests().pop().unwrap();
        let outputs = request.outputs().clone();
        let signatures = outputs
            .iter()
            .map(|output| BlindSignature {
                amount: output.amount,
                keyset_id,
                c: crate::dhke::sign_message(
                    &client.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&output.amount],
                    &output.blinded_secret,
                )
                .unwrap(),
                dleq: None,
            })
            .collect();
        (
            wallet,
            client,
            saga,
            RestoreResponse {
                outputs,
                signatures,
            },
        )
    }

    fn stage_spent(client: &MockMintConnector, inputs: &[cdk_common::wallet::ProofInfo]) {
        client.set_post_swap_response(Err(crate::Error::TokenAlreadySpent));
        client.set_check_state_response(Ok(CheckStateResponse {
            states: inputs
                .iter()
                .map(|proof| (proof.y, State::Spent).into())
                .collect(),
        }));
    }

    #[tokio::test]
    async fn receive_recovery_unavailable_restore_preserves_pending() {
        for error in [
            crate::Error::Timeout,
            crate::Error::HttpError(Some(404), "unavailable".to_owned()),
        ] {
            let db = create_test_db().await;
            let (wallet, client, saga, _) = interrupted_receive(db.clone()).await;
            let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
            stage_spent(&client, &inputs);
            client._set_restore_response(Err(error));
            let report = wallet.recover_incomplete_sagas().await.unwrap();
            assert_eq!(report.recovered, 0);
            assert_eq!(report.compensated, 0);
            assert!(db.get_saga(&saga.id).await.unwrap().is_some());
            assert_eq!(
                db.get_reserved_proofs(&saga.id).await.unwrap().len(),
                inputs.len()
            );
            assert_eq!(
                db.list_transactions(None, None, None).await.unwrap()[0].status,
                TransactionStatus::Pending
            );
            assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        }
    }

    #[tokio::test]
    async fn receive_recovery_missing_restore_data_preserves_pending() {
        let db = create_test_db().await;
        let (wallet, client, mut saga, _) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        if let OperationData::Receive(data) = &mut saga.data {
            data.blinded_messages = None;
        }
        saga.update_state(saga.state);
        assert!(db.update_saga(saga.clone()).await.unwrap());
        stage_spent(&client, &inputs);
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
    }

    #[tokio::test]
    async fn receive_recovery_missing_inputs_is_not_receipt_evidence() {
        let db = create_test_db().await;
        let (wallet, client, saga, _) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        db.update_proofs(vec![], inputs.iter().map(|p| p.y).collect())
            .await
            .unwrap();
        client._set_restore_response(Err(crate::Error::Timeout));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
    }

    #[tokio::test]
    async fn receive_recovery_missing_inputs_restores_verified_output_states() {
        for state in [State::Unspent, State::Spent, State::Pending] {
            let db = create_test_db().await;
            let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
            let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
            db.update_proofs(vec![], inputs.iter().map(|p| p.y).collect())
                .await
                .unwrap();
            *client.recovered_output_state.lock().unwrap() = Some(state);
            client._set_restore_response(Ok(response));
            assert_eq!(
                wallet.recover_incomplete_sagas().await.unwrap().recovered,
                1
            );
            let balance = if state == State::Unspent {
                Amount::from(8)
            } else {
                Amount::ZERO
            };
            assert_eq!(wallet.total_balance().await.unwrap(), balance);
            let proofs = db.get_proofs(None, None, None, None).await.unwrap();
            assert!(!proofs.is_empty());
            assert!(proofs.iter().all(|p| p.state == state));
            assert_eq!(
                db.list_transactions(None, None, None).await.unwrap()[0].status,
                TransactionStatus::Completed
            );
            assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
            assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), proofs);
        }
    }

    #[tokio::test]
    async fn receive_recovery_after_retry_preserves_original_receipt() {
        let db = create_test_db().await;
        let (wallet, client, original, response) = interrupted_receive(db.clone()).await;
        let token = match &original.data {
            OperationData::Receive(data) => data.token.as_ref().unwrap(),
            _ => unreachable!(),
        };
        client.set_post_swap_response(Err(crate::Error::TokenAlreadySpent));
        assert!(matches!(
            wallet
                .receive(token, crate::wallet::ReceiveOptions::default())
                .await,
            Err(crate::Error::TokenAlreadySpent)
        ));
        assert!(db
            .get_reserved_proofs(&original.id)
            .await
            .unwrap()
            .is_empty());
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.resume_receive_saga(&original).await.unwrap(),
            RecoveryAction::Recovered
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        let other = db.get_incomplete_sagas().await.unwrap().pop().unwrap();
        let inputs = db.get_reserved_proofs(&other.id).await.unwrap();
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(RestoreResponse {
            outputs: vec![],
            signatures: vec![],
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(
            transactions
                .iter()
                .filter(|t| t.status == TransactionStatus::Completed)
                .count(),
            1
        );
        assert_eq!(
            wallet.resume_receive_saga(&original).await.unwrap(),
            RecoveryAction::Skipped
        );
    }

    #[tokio::test]
    async fn receive_recovery_invalid_output_states_remains_retryable() {
        use crate::nuts::PreMintSecrets;

        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        db.update_proofs(vec![], inputs.iter().map(|p| p.y).collect())
            .await
            .unwrap();
        let data = match &saga.data {
            OperationData::Receive(data) => data,
            _ => unreachable!(),
        };
        let premints = PreMintSecrets::restore_batch(
            response.outputs[0].keyset_id,
            &[42; 64],
            data.counter_start.unwrap(),
            data.counter_end.unwrap(),
        )
        .unwrap();
        let states: Vec<ProofState> = premints
            .secrets
            .iter()
            .map(|p| {
                (
                    crate::dhke::hash_to_curve(&p.secret.to_bytes()).unwrap(),
                    State::Unspent,
                )
                    .into()
            })
            .collect();
        let mut duplicate = states.clone();
        duplicate.push(states[0].clone());
        let mut unknown = states.clone();
        unknown[0].y = inputs[0].y;
        let mut invalid = states.clone();
        invalid[0].state = State::Reserved;
        for response_states in [vec![], duplicate, unknown, invalid] {
            client._set_restore_response(Ok(response.clone()));
            client.set_check_state_response(Ok(CheckStateResponse {
                states: response_states,
            }));
            let report = wallet.recover_incomplete_sagas().await.unwrap();
            assert_eq!(report.failed, 1);
            assert_eq!(report.recovered, 0);
            assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
            assert!(db.get_saga(&saga.id).await.unwrap().is_some());
            assert_eq!(
                db.list_transactions(None, None, None).await.unwrap()[0].status,
                TransactionStatus::Pending
            );
        }
        client._set_restore_response(Ok(response.clone()));
        client.set_check_state_response(Err(crate::Error::Timeout));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
    }

    #[tokio::test]
    async fn receive_recovery_lost_response_restores_once() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        for proof in &proofs {
            crate::dhke::verify_message(
                &client.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&proof.proof.amount],
                proof.proof.c,
                &proof.proof.secret.to_bytes(),
            )
            .unwrap();
        }
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Completed
        );
        for _ in 0..3 {
            assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
        }
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), proofs);
    }

    #[tokio::test]
    async fn receive_recovery_concurrent_stale_restore_preserves_spent_outputs() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response.clone()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *client.restore_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let other_client = Arc::new(MockMintConnector::new());
        other_client.enable_mint_signing();
        *other_client.recovered_output_state.lock().unwrap() = Some(State::Unspent);
        let other = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            other_client.clone(),
            [42; 64],
        )
        .await;
        stage_spent(&other_client, &inputs);
        other_client._set_restore_response(Ok(response));
        let stalled = tokio::spawn(async move { wallet.recover_incomplete_sagas().await.unwrap() });
        entered.notified().await;
        assert_eq!(other.recover_incomplete_sagas().await.unwrap().recovered, 1);
        let outputs = db.get_proofs(None, None, None, None).await.unwrap();
        db.update_proofs_state(outputs.iter().map(|p| p.y).collect(), State::Spent)
            .await
            .unwrap();
        release.add_permits(1);
        assert_eq!(stalled.await.unwrap().recovered, 0);
        assert_eq!(other.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db
            .get_proofs(None, None, None, None)
            .await
            .unwrap()
            .iter()
            .all(|p| p.state == State::Spent));
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Completed
        );
    }

    #[tokio::test]
    async fn receive_recovery_recognizes_persisted_outputs_without_rewriting_them() {
        use crate::wallet::recovery::RecoveryHelpers;

        for state in [State::Unspent, State::Spent, State::Reserved] {
            let db = create_test_db().await;
            let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
            let data = match &saga.data {
                OperationData::Receive(data) => data,
                _ => unreachable!(),
            };
            client._set_restore_response(Ok(response));
            let mut proofs = wallet
                .restore_outputs(
                    &saga.id,
                    "Receive",
                    data.blinded_messages.as_deref(),
                    data.counter_start,
                    data.counter_end,
                )
                .await
                .unwrap()
                .unwrap();
            for proof in &mut proofs {
                proof.state = state;
                if state == State::Reserved {
                    proof.used_by_operation = Some(uuid::Uuid::new_v4());
                }
            }
            let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
            db.update_proofs(proofs.clone(), inputs.iter().map(|p| p.y).collect())
                .await
                .unwrap();
            assert_eq!(
                wallet.recover_incomplete_sagas().await.unwrap().recovered,
                1
            );
            assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), proofs);
            assert!(db.get_saga(&saga.id).await.unwrap().is_none());
            assert_eq!(
                db.list_transactions(None, None, None).await.unwrap()[0].status,
                TransactionStatus::Completed
            );
        }
    }

    #[tokio::test]
    async fn receive_recovery_sqlite_rolls_back_and_recovers_after_reopen() {
        let path =
            std::env::temp_dir().join(format!("cdk-receive-{}.sqlite", uuid::Uuid::new_v4()));
        let db = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(path.clone())
                .await
                .unwrap(),
        );
        let (wallet, client, saga, response) = interrupted_receive_with_options(
            db.clone(),
            crate::wallet::ReceiveOptions {
                amount_split_target: crate::amount::SplitTarget::Values(vec![4.into(), 4.into()]),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(response.outputs.len(), 2);
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_receive_output BEFORE INSERT ON proof WHEN NEW.used_by_operation IS NULL AND (SELECT COUNT(*) FROM proof WHERE used_by_operation IS NULL) > 0 BEGIN SELECT RAISE(ABORT, 'injected persistence failure'); END;").unwrap();
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response.clone()));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(db.get_reserved_proofs(&saga.id).await.unwrap(), inputs);
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), inputs);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        tokio::task::spawn_blocking(move || {
            connection.execute_batch("DROP TRIGGER reject_receive_output;")
        })
        .await
        .unwrap()
        .unwrap();
        drop(wallet);
        drop(db);
        let db = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(path.clone())
                .await
                .unwrap(),
        );
        let wallet = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            client.clone(),
            [42; 64],
        )
        .await;
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        drop(wallet);
        drop(db);
        let db = Arc::new(cdk_sqlite::WalletSqliteDatabase::new(path).await.unwrap());
        let wallet = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            client,
            [42; 64],
        )
        .await;
        for _ in 0..3 {
            assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
        }
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), proofs);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Completed
        );
    }

    #[tokio::test]
    async fn receive_recovery_replays_valid_signatures() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn receive_recovery_valid_replay_without_optional_checkstate() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        *client.recovered_output_state.lock().unwrap() = None;
        client.set_check_state_response(Err(crate::Error::Timeout));
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    fn stage_general_restore(client: &MockMintConnector, response: RestoreResponse) {
        let mut responses = client.restore_responses.lock().unwrap();
        responses.push_back(Ok(response));
        for _ in 0..3 {
            responses.push_back(Ok(RestoreResponse {
                outputs: vec![],
                signatures: vec![],
            }));
        }
    }

    #[tokio::test]
    async fn general_restore_full_outputs_complete_receive_without_network() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        stage_general_restore(&client, response);
        wallet.restore().await.unwrap();
        let before = db.get_proofs(None, None, None, None).await.unwrap();
        client.set_post_swap_response(Err(crate::Error::Timeout));
        client._set_restore_response(Err(crate::Error::Timeout));
        client.set_check_state_response(Err(crate::Error::Timeout));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        let outputs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(outputs.iter().all(|p| before.contains(p)));
        assert_eq!(
            Amount::try_sum(outputs.iter().map(|p| p.proof.amount)).unwrap(),
            Amount::from(8)
        );
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn general_restore_partial_outputs_keep_conservative_recovery() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive_with_options(
            db.clone(),
            crate::wallet::ReceiveOptions {
                amount_split_target: crate::amount::SplitTarget::Values(vec![4.into(), 4.into()]),
                ..Default::default()
            },
        )
        .await;
        let partial = RestoreResponse {
            outputs: response.outputs[..1].to_vec(),
            signatures: response.signatures[..1].to_vec(),
        };
        stage_general_restore(&client, partial);
        wallet.restore().await.unwrap();
        let before = db.get_proofs(None, None, None, None).await.unwrap();
        for _ in 0..2 {
            client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
                signatures: response.signatures.clone(),
            }));
            client.set_check_state_response(Err(crate::Error::Timeout));
            assert_eq!(
                wallet.recover_incomplete_sagas().await.unwrap().recovered,
                0
            );
            assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(4));
            assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), before);
            assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        }
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn general_restore_spend_reopen_does_not_restore_spendable_receive() {
        let path = std::env::temp_dir().join(format!(
            "cdk-restore-receive-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(path.clone())
                .await
                .unwrap(),
        );
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        stage_general_restore(&client, response.clone());
        wallet.restore().await.unwrap();
        let materialized = db.get_saga(&saga.id).await.unwrap().unwrap();
        assert_eq!(materialized.version, saga.version + 1);
        assert!(
            matches!(materialized.data, OperationData::Receive(data) if !data.atomic_settlement)
        );
        let proofs = wallet.get_unspent_proofs().await.unwrap();
        assert!(!proofs.is_empty());
        *client.recovered_output_state.lock().unwrap() = Some(State::Spent);
        wallet.check_proofs_spent(proofs).await.unwrap();
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        drop(wallet);
        drop(db);
        let db = Arc::new(cdk_sqlite::WalletSqliteDatabase::new(path).await.unwrap());
        let wallet = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            client.clone(),
            [42; 64],
        )
        .await;
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures.clone(),
        }));
        client.set_check_state_response(Err(crate::Error::Timeout));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db
            .get_proofs(None, None, None, None)
            .await
            .unwrap()
            .iter()
            .all(|p| p.state == State::Spent));
        assert_eq!(
            db.get_transaction(cdk_common::wallet::TransactionId::from_saga_id(saga.id))
                .await
                .unwrap()
                .unwrap()
                .status,
            TransactionStatus::Completed
        );
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn general_restore_failure_rolls_back_provenance_with_proofs() {
        let path = std::env::temp_dir().join(format!(
            "cdk-restore-rollback-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(path.clone())
                .await
                .unwrap(),
        );
        let (wallet, client, saga, response) = interrupted_receive_with_options(
            db.clone(),
            crate::wallet::ReceiveOptions {
                amount_split_target: crate::amount::SplitTarget::Values(vec![4.into(), 4.into()]),
                ..Default::default()
            },
        )
        .await;
        let observer = rusqlite::Connection::open(path).unwrap();
        observer.execute_batch("CREATE TRIGGER reject_restore BEFORE INSERT ON proof WHEN NEW.used_by_operation IS NULL AND (SELECT COUNT(*) FROM proof WHERE used_by_operation IS NULL) > 0 BEGIN SELECT RAISE(ABORT, 'persistence failure'); END;").unwrap();
        stage_general_restore(&client, response.clone());
        assert!(wallet.restore().await.is_err());
        assert_eq!(db.get_saga(&saga.id).await.unwrap(), Some(saga));
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        tokio::task::spawn_blocking(move || observer.execute_batch("DROP TRIGGER reject_restore;"))
            .await
            .unwrap()
            .unwrap();
        client.set_check_state_response(Err(crate::Error::Timeout));
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
    }

    #[tokio::test]
    async fn general_restore_rejects_receive_transition_after_snapshot() {
        let db = create_test_db().await;
        let (wallet, client, mut saga, response) = interrupted_receive(db.clone()).await;
        let observer = wallet.clone();
        let before = db.get_proofs(None, None, None, None).await.unwrap();
        saga.update_state(WalletSagaState::Receive(ReceiveSagaState::ProofsPending));
        assert!(db.update_saga(saga.clone()).await.unwrap());
        stage_general_restore(&client, response);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *client.restore_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let restoring = tokio::spawn(async move { wallet.restore().await });
        entered.notified().await;
        saga.update_state(WalletSagaState::Receive(ReceiveSagaState::SwapRequested));
        assert!(db.update_saga(saga.clone()).await.unwrap());
        *client.restore_gate.lock().unwrap() = None;
        release.add_permits(1);
        assert!(matches!(
            restoring.await.unwrap(),
            Err(crate::Error::ConcurrentUpdate)
        ));
        assert_eq!(db.get_saga(&saga.id).await.unwrap(), Some(saga));
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), before);
        assert_eq!(observer.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn general_restore_rejects_output_allocation_after_snapshot() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let observer = wallet.clone();
        let before = db.get_proofs(None, None, None, None).await.unwrap();
        let keyset_id = response.outputs[0].keyset_id;
        stage_general_restore(&client, response);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *client.restore_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let restoring = tokio::spawn(async move { wallet.restore().await });
        entered.notified().await;
        db.increment_keyset_counter(&keyset_id, 1).await.unwrap();
        let advanced_counter = db.increment_keyset_counter(&keyset_id, 0).await.unwrap();
        *client.restore_gate.lock().unwrap() = None;
        release.add_permits(1);
        assert!(matches!(
            restoring.await.unwrap(),
            Err(crate::Error::ConcurrentUpdate)
        ));
        assert_eq!(db.get_saga(&saga.id).await.unwrap(), Some(saga));
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), before);
        assert_eq!(observer.total_balance().await.unwrap(), Amount::ZERO);
        assert_eq!(
            db.increment_keyset_counter(&keyset_id, 0).await.unwrap(),
            advanced_counter
        );
        assert!(db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn general_restore_invalidates_inflight_receive_recovery() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        client.set_post_swap_response(Err(crate::Error::TokenAlreadySpent));
        client._set_restore_response(Ok(response.clone()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        *client.restore_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let stalled = tokio::spawn(async move { wallet.recover_incomplete_sagas().await.unwrap() });
        entered.notified().await;
        let other_client = Arc::new(MockMintConnector::new());
        other_client.enable_mint_signing();
        *other_client.recovered_output_state.lock().unwrap() = Some(State::Unspent);
        let other = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            other_client.clone(),
            [42; 64],
        )
        .await;
        stage_general_restore(&other_client, response);
        other.restore().await.unwrap();
        *other_client.recovered_output_state.lock().unwrap() = Some(State::Spent);
        other
            .check_proofs_spent(other.get_unspent_proofs().await.unwrap())
            .await
            .unwrap();
        release.add_permits(1);
        assert_eq!(stalled.await.unwrap().recovered, 0);
        assert_eq!(other.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn receive_recovery_valid_restore_without_optional_checkstate() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        *client.recovered_output_state.lock().unwrap() = None;
        client.set_check_state_response(Err(crate::Error::Timeout));
        client.set_post_swap_response(Err(crate::Error::TokenAlreadySpent));
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn receive_recovery_duplicate_cached_signatures_remains_retryable() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive_with_options(
            db.clone(),
            crate::wallet::ReceiveOptions {
                amount_split_target: crate::amount::SplitTarget::Values(vec![4.into(), 4.into()]),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(response.signatures.len(), 2);
        let duplicate = response.signatures[0].clone();
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: vec![duplicate.clone(), duplicate],
        }));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.failed, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn receive_recovery_legacy_partial_persistence_does_not_reset_spent_outputs() {
        use crate::wallet::recovery::RecoveryHelpers;

        let db = create_test_db().await;
        let (wallet, client, mut saga, response) = interrupted_receive_with_options(
            db.clone(),
            crate::wallet::ReceiveOptions {
                amount_split_target: crate::amount::SplitTarget::Values(vec![4.into(), 4.into()]),
                ..Default::default()
            },
        )
        .await;
        let data = match &mut saga.data {
            OperationData::Receive(data) => data,
            _ => unreachable!(),
        };
        data.atomic_settlement = false;
        client._set_restore_response(Ok(response.clone()));
        let mut outputs = wallet
            .restore_outputs(
                &saga.id,
                "Receive",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await
            .unwrap()
            .unwrap();
        let mut spent = outputs.remove(0);
        spent.state = State::Spent;
        db.update_proofs(vec![spent.clone()], vec![]).await.unwrap();
        saga.update_state(saga.state);
        assert!(db.update_saga(saga.clone()).await.unwrap());
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        assert!(!inputs.is_empty());
        for _ in 0..2 {
            client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
                signatures: response.signatures.clone(),
            }));
            client.set_check_state_response(Err(crate::Error::Timeout));
            assert_eq!(
                wallet.recover_incomplete_sagas().await.unwrap().recovered,
                0
            );
            assert!(db.get_saga(&saga.id).await.unwrap().is_some());
            assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
            assert_eq!(db.get_reserved_proofs(&saga.id).await.unwrap(), inputs);
            assert_eq!(
                db.get_proofs_by_ys(vec![spent.y]).await.unwrap(),
                vec![spent.clone()]
            );
            assert_eq!(
                db.list_transactions(None, None, None).await.unwrap()[0].status,
                TransactionStatus::Pending
            );
        }
        *client.recovered_output_state.lock().unwrap() = Some(State::Spent);
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        let stored = db.get_proofs(None, None, None, None).await.unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored.iter().all(|p| p.state == State::Spent));
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn receive_recovery_completed_orphan_with_inputs_does_not_reset_spent_outputs() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let mut transaction = db
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .pop()
            .unwrap();
        transaction.status = TransactionStatus::Completed;
        db.add_transaction(transaction).await.unwrap();
        *client.recovered_output_state.lock().unwrap() = Some(State::Spent);
        client.set_post_swap_response(Ok(crate::nuts::SwapResponse {
            signatures: response.signatures,
        }));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        let outputs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(!outputs.is_empty());
        assert!(outputs.iter().all(|p| p.state == State::Spent));
    }

    #[tokio::test]
    async fn receive_recovery_reloads_stale_saga_state() {
        let db = create_test_db().await;
        let (wallet, client, mut stale, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&stale.id).await.unwrap();
        stale.state = WalletSagaState::Receive(ReceiveSagaState::ProofsPending);
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.resume_receive_saga(&stale).await.unwrap(),
            RecoveryAction::Recovered
        );
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
    }

    #[tokio::test]
    async fn receive_recovery_completed_orphan_does_not_report_new_receipt() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        db.add_saga(saga.clone()).await.unwrap();
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert_eq!(db.get_proofs(None, None, None, None).await.unwrap(), proofs);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
    }

    #[tokio::test]
    async fn receive_recovery_completed_orphan_retains_and_restores_missing_proofs() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        db.update_proofs(vec![], inputs.iter().map(|p| p.y).collect())
            .await
            .unwrap();
        let mut transaction = db
            .list_transactions(None, None, None)
            .await
            .unwrap()
            .pop()
            .unwrap();
        transaction.status = TransactionStatus::Completed;
        db.add_transaction(transaction).await.unwrap();
        client._set_restore_response(Err(crate::Error::Timeout));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            0
        );
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        client._set_restore_response(Ok(response));
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
        assert!(db.get_saga(&saga.id).await.unwrap().is_none());
        assert!(wallet.recover_incomplete_sagas().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn receive_recovery_reconstructs_historical_amount_from_outputs() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        db.remove_transaction(cdk_common::wallet::TransactionId::from_saga_id(saga.id))
            .await
            .unwrap();
        let keyset_id = {
            let mut keysets = client.keysets.lock().unwrap();
            keysets[0].input_fee_ppk = 1000;
            keysets[0].id
        };
        db.add_mint_keysets(
            test_mint_url(),
            vec![cdk_common::nuts::KeySetInfo {
                id: keyset_id,
                unit: CurrencyUnit::Sat,
                active: true,
                input_fee_ppk: 1000,
                final_expiry: None,
            }],
        )
        .await
        .unwrap();
        drop(wallet);
        let wallet = crate::wallet::test_utils::create_test_wallet_with_mock_seed(
            db.clone(),
            client.clone(),
            [42; 64],
        )
        .await;
        let proofs = inputs.iter().map(|p| p.proof.clone()).collect::<Vec<_>>();
        assert!(wallet.get_proofs_fee(&proofs).await.unwrap().total > Amount::ZERO);
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions[0].status, TransactionStatus::Completed);
        assert_eq!(transactions[0].amount, Amount::from(8));
        assert_eq!(transactions[0].fee, Amount::ZERO);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(8));
    }

    #[tokio::test]
    async fn receive_recovery_invalid_signature_metadata_remains_retryable() {
        let db = create_test_db().await;
        let (wallet, client, saga, response) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        let mut invalid = response.clone();
        invalid.signatures[0].amount += Amount::from(1);
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(invalid));
        assert_eq!(wallet.recover_incomplete_sagas().await.unwrap().failed, 1);
        assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
        assert_eq!(db.get_reserved_proofs(&saga.id).await.unwrap(), inputs);
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
        stage_spent(&client, &inputs);
        client._set_restore_response(Ok(response));
        assert_eq!(
            wallet.recover_incomplete_sagas().await.unwrap().recovered,
            1
        );
    }

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
                atomic_settlement: false,
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
    async fn failed_receive_transaction_stays_failed_during_orphan_cleanup() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Receive(ReceiveSagaState::SwapRequested),
            Amount::from(100),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                atomic_settlement: false,
                token: Some("test_token".to_string()),
                counter_start: Some(0),
                counter_end: Some(1),
                amount: Some(Amount::from(100)),
                blinded_messages: Some(vec![]),
            }),
        );
        db.add_saga(saga).await.unwrap();
        db.add_transaction(Transaction {
            mint_url,
            direction: TransactionDirection::Incoming,
            amount: Amount::from(100),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![],
            timestamp: 0,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status: TransactionStatus::Failed,
        })
        .await
        .unwrap();

        let wallet =
            create_test_wallet_with_mock(db.clone(), Arc::new(MockMintConnector::new())).await;
        let action = wallet
            .resume_receive_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await
            .expect("orphaned saga should be cleaned up");

        assert_eq!(action, RecoveryAction::Compensated);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].status, TransactionStatus::Failed);
    }

    #[tokio::test]
    async fn receive_recovery_unspent_inputs_without_output_data_is_deferred() {
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
                atomic_settlement: false,
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
        assert_eq!(recovery_action, RecoveryAction::Skipped);
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
        assert_eq!(db.get_reserved_proofs(&saga_id).await.unwrap().len(), 1);

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].status, TransactionStatus::Pending);
    }

    #[tokio::test]
    async fn receive_recovery_spent_inputs_without_output_data_remains_pending() {
        let db = create_test_db().await;
        let (wallet, client, mut saga, _) = interrupted_receive(db.clone()).await;
        let inputs = db.get_reserved_proofs(&saga.id).await.unwrap();
        if let OperationData::Receive(data) = &mut saga.data {
            data.blinded_messages = Some(vec![]);
        }
        saga.update_state(saga.state);
        assert!(db.update_saga(saga.clone()).await.unwrap());
        stage_spent(&client, &inputs);
        let report = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(report.recovered, 0);
        assert_eq!(db.get_reserved_proofs(&saga.id).await.unwrap(), inputs);
        assert!(db.get_saga(&saga.id).await.unwrap().is_some());
        assert_eq!(
            db.list_transactions(None, None, None).await.unwrap()[0].status,
            TransactionStatus::Pending
        );
    }
}
