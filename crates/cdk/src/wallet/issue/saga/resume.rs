//! Resume logic for issue (mint) sagas after crash recovery.
//!
//! This module handles resuming incomplete issue sagas that were interrupted
//! by a crash. It attempts to recover outputs using stored blinded messages.
//!
//! # Recovery Strategy
//!
//! For `MintRequested` state, we use a replay-first strategy:
//! 1. **Replay**: Attempt to replay the original `post_mint` request.
//!    If the mint cached the response (NUT-19), we get signatures immediately.
//! 2. **Fallback**: If replay fails, use `/restore` to recover outputs.

use std::collections::HashMap;

use cdk_common::wallet::{
    IssueSagaState, MintOperationData, OperationData, ProofInfo, Transaction, TransactionDirection,
    TransactionId, TransactionStatus, WalletSaga,
};
use cdk_common::{Amount, PaymentMethod};
use tracing::instrument;

use crate::dhke::{construct_proofs, hash_to_curve};
use crate::nuts::{MintRequest, PreMintSecrets, State};
use crate::util::unix_time;
use crate::wallet::blind_signature::{
    validate_mint_response_signatures, SignatureAmountValidation,
};
use crate::wallet::issue::saga::compensation::ReleaseMintQuote;
use crate::wallet::issue::saga::state::PreparedMintRequest;
use crate::wallet::recovery::{OutputRecoveryResult, RecoveryAction};
use crate::wallet::saga::CompensatingAction;
use crate::{Error, Wallet};

fn is_mint_limit_error(error: &Error) -> bool {
    matches!(
        error,
        Error::MaxInputsExceeded { .. } | Error::MaxOutputsExceeded { .. }
    )
}

impl Wallet {
    /// Resume an incomplete issue saga after crash recovery.
    ///
    /// Recovery depends on state:
    /// - SecretsPrepared: No mint request sent, safe to compensate.
    /// - MintRequested: Mint request sent, attempt to recover outputs.
    #[instrument(skip(self, saga))]
    pub(crate) async fn resume_issue_saga(
        &self,
        saga: &WalletSaga,
    ) -> Result<RecoveryAction, Error> {
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
                self.update_transaction_status_by_saga_id(saga.id, TransactionStatus::Failed)
                    .await?;
                self.compensate_issue(&saga.id).await?;
                Ok(RecoveryAction::Compensated)
            }
            IssueSagaState::MintRequested => {
                // Mint request was sent - try to recover outputs
                tracing::info!(
                    "Issue saga {} in MintRequested state - attempting recovery",
                    saga.id
                );
                // Return the result directly (RecoveryAction)
                self.complete_issue_from_restore(&saga.id, data).await
            }
        }
    }

    /// Complete an issue by first trying replay, then falling back to restore.
    /// Replay leverages NUT-19 caching.
    async fn complete_issue_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<RecoveryAction, Error> {
        let quote_ids = data.quote_ids();
        self.ensure_pending_issue_transaction(saga_id, data).await?;

        // Try replay first
        let replay_result = self.try_replay_mint(saga_id, data).await;
        if let Err(e) = &replay_result {
            if is_mint_limit_error(e) {
                self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Failed)
                    .await?;
                self.compensate_issue(saga_id).await?;
            }
        }

        if let Some(proofs) = replay_result? {
            // Replay succeeded - save proofs and clean up
            self.localstore
                .update_proofs(proofs.clone(), vec![])
                .await?;

            self.record_recovered_issue_transaction(saga_id, &quote_ids, &proofs)
                .await?;
            self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Completed)
                .await?;

            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        // Replay failed, fall back to /restore
        let recovery_result = self
            .restore_outputs_with_result(
                saga_id,
                "Issue",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        match recovery_result {
            OutputRecoveryResult::Restored(proofs) => {
                // Issue has no input proofs to remove - just add the recovered proofs
                self.localstore
                    .update_proofs(proofs.clone(), vec![])
                    .await?;

                self.record_recovered_issue_transaction(saga_id, &quote_ids, &proofs)
                    .await?;
                self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Completed)
                    .await?;

                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            OutputRecoveryResult::EmptyResponse => {
                // The mint definitively has no signatures for these outputs.
                tracing::warn!(
                    "Issue saga {} - mint returned no restorable outputs",
                    saga_id
                );
                self.update_transaction_status_by_saga_id(*saga_id, TransactionStatus::Failed)
                    .await?;
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Compensated)
            }
            OutputRecoveryResult::Unavailable => {
                tracing::warn!(
                    "Issue saga {} - output recovery is unavailable, keeping it pending",
                    saga_id
                );
                Ok(RecoveryAction::Skipped)
            }
        }
    }

    /// Recreate a pending issue transaction if a crash happened after the saga
    /// write-ahead update but before the transaction was stored.
    async fn ensure_pending_issue_transaction(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<(), Error> {
        let blinded_messages = match data.blinded_messages.as_deref() {
            Some(messages) if !messages.is_empty() => messages,
            _ => return Ok(()),
        };
        let (counter_start, counter_end) = match (data.counter_start, data.counter_end) {
            (Some(start), Some(end)) => (start, end),
            _ => return Ok(()),
        };

        let premint_secrets = PreMintSecrets::restore_batch(
            blinded_messages[0].keyset_id,
            &self.seed,
            counter_start,
            counter_end,
        )?;
        let ys = premint_secrets
            .secrets
            .iter()
            .map(|pre_mint| hash_to_curve(pre_mint.secret.as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;

        if self
            .localstore
            .get_transaction(TransactionId::new(ys.clone()))
            .await?
            .is_some()
        {
            return Ok(());
        }

        let quote_id = data.quote_ids().first().cloned();
        let quote = match quote_id.as_deref() {
            Some(quote_id) => self.localstore.get_mint_quote(quote_id).await?,
            None => None,
        };

        self.upsert_transaction(Transaction {
            mint_url: self.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: data.amount,
            fee: Amount::ZERO,
            unit: self.unit.clone(),
            ys,
            timestamp: unix_time(),
            memo: None,
            metadata: HashMap::new(),
            quote_id,
            payment_request: quote.as_ref().map(|quote| quote.request.clone()),
            payment_proof: None,
            payment_method: quote.map(|quote| quote.payment_method),
            saga_id: Some(*saga_id),
            status: TransactionStatus::Pending,
        })
        .await?;

        Ok(())
    }

    /// Record a transaction for recovered issue proofs.
    /// Skipped if quote not found (recovery still succeeds).
    /// For batch operations, records transaction using the first quote.
    async fn record_recovered_issue_transaction(
        &self,
        saga_id: &uuid::Uuid,
        quote_ids: &[String],
        proofs: &[ProofInfo],
    ) -> Result<(), Error> {
        // Use the first quote for transaction recording
        let quote_id = quote_ids.first().ok_or(Error::UnknownQuote)?;

        // Get and update quote state from mint
        let quote = match self.localstore.get_mint_quote(quote_id).await? {
            Some(mut q) => {
                // Update state from mint
                if let Err(e) = self.check_state(&mut q).await {
                    tracing::warn!(
                        "Failed to check quote state for transaction recording: {}",
                        e
                    );
                }
                // Save updated quote state
                if let Err(e) = self.localstore.add_mint_quote(q.clone()).await {
                    tracing::warn!("Failed to save updated quote state: {}", e);
                }
                q
            }
            None => {
                tracing::warn!(
                    "Issue saga {} - quote {} not found, skipping transaction recording",
                    saga_id,
                    quote_id
                );
                return Ok(());
            }
        };

        let minted_amount = proofs
            .iter()
            .fold(Amount::ZERO, |acc, p| acc + p.proof.amount);
        let ys: Vec<_> = proofs.iter().map(|p| p.y).collect();

        self.upsert_transaction(Transaction {
            mint_url: self.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: minted_amount,
            fee: Amount::ZERO,
            unit: self.unit.clone(),
            ys,
            timestamp: unix_time(),
            memo: None,
            metadata: HashMap::new(),
            quote_id: Some(quote_id.to_string()),
            payment_request: Some(quote.request.clone()),
            payment_proof: None,
            payment_method: Some(quote.payment_method.clone()),
            saga_id: Some(*saga_id),
            status: TransactionStatus::Completed,
        })
        .await?;

        Ok(())
    }

    /// Attempt to replay the original mint request.
    ///
    /// This leverages NUT-19 caching: if the mint has a cached response for this
    /// exact request, it will return the signatures immediately.
    ///
    /// For batch operations, uses post_batch_mint instead of post_mint.
    ///
    /// Returns:
    /// - `Ok(Some(proofs))` if replay succeeded and we got signatures
    /// - `Ok(None)` if replay failed (fall back to /restore)
    /// - `Err` only for unrecoverable errors
    async fn try_replay_mint(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
        use crate::nuts::BatchMintRequest;

        // We need blinded messages to reconstruct the request
        let blinded_messages = match &data.blinded_messages {
            Some(bm) if !bm.is_empty() => bm,
            _ => {
                tracing::debug!(
                    "Issue saga {} - no blinded messages stored, cannot replay",
                    saga_id
                );
                return Ok(None);
            }
        };

        let quote_ids = data.quote_ids();
        let is_batch = data.is_batch();

        if is_batch {
            // Batch replay: need to get all quotes and construct BatchMintRequest
            let mut quote_infos: Vec<cdk_common::wallet::MintQuote> = Vec::new();
            let mut payment_method: Option<PaymentMethod> = None;

            for quote_id in &quote_ids {
                let quote = match self.localstore.get_mint_quote(quote_id).await? {
                    Some(q) => q,
                    None => {
                        tracing::debug!(
                            "Issue saga {} - mint quote {} not found, cannot replay",
                            saga_id,
                            quote_id
                        );
                        return Ok(None);
                    }
                };
                payment_method = Some(quote.payment_method.clone());
                quote_infos.push(quote);
            }

            let payment_method = payment_method.ok_or(Error::UnknownQuote)?;

            // Build quote amounts
            let quote_amounts: Vec<Amount> =
                quote_infos.iter().map(|q| q.amount_mintable()).collect();

            // Construct batch mint request
            let mut batch_request = BatchMintRequest {
                quotes: quote_ids.clone(),
                quote_amounts: Some(quote_amounts),
                outputs: blinded_messages.clone(),
                signatures: None,
            };

            // Build signatures for locked quotes (NUT-20)
            let mut signatures: Vec<Option<String>> = Vec::new();
            for quote in &quote_infos {
                if let Some(secret_key) = self.mint_quote_signing_key(quote).await? {
                    let sig = batch_request
                        .sign_quote(&quote.id, &secret_key)
                        .map_err(|e| Error::Custom(format!("NUT-20 signing failed: {}", e)))?;
                    signatures.push(Some(sig));
                } else {
                    signatures.push(None);
                }
            }

            let has_locked = signatures.iter().any(Option::is_some);
            let signatures_to_send = if has_locked { Some(signatures) } else { None };
            batch_request.signatures = signatures_to_send;

            tracing::info!(
                "Issue saga {} - attempting replay of post_batch_mint request",
                saga_id
            );

            let mint_request = PreparedMintRequest::Batch {
                quote_ids: quote_ids.clone(),
                quote_infos: quote_infos.clone(),
                request: batch_request,
            };

            // Attempt batch replay
            let mint_response = match super::post_mint_request_with_legacy_fallback(
                self,
                &payment_method,
                &mint_request,
            )
            .await
            {
                Ok(response) => response,
                Err(e) => {
                    if is_mint_limit_error(&e) {
                        tracing::warn!(
                            "Issue saga {} - batch replay failed with mint limit: {}",
                            saga_id,
                            e
                        );
                        return Err(e);
                    }

                    tracing::info!(
                        "Issue saga {} - batch replay failed ({}), falling back to restore",
                        saga_id,
                        e
                    );
                    return Ok(None);
                }
            };

            // Continue with proof construction (same as single)
            let (counter_start, counter_end) = match (data.counter_start, data.counter_end) {
                (Some(start), Some(end)) => (start, end),
                _ => {
                    tracing::warn!(
                        "Issue saga {} - no counter range stored, cannot construct proofs",
                        saga_id
                    );
                    return Ok(None);
                }
            };

            let keyset_id = blinded_messages[0].keyset_id;

            let premint_secrets = crate::nuts::PreMintSecrets::restore_batch(
                keyset_id,
                &self.seed,
                counter_start,
                counter_end,
            )?;

            let keys = self.keyset(keyset_id).await?.keys;

            validate_mint_response_signatures(
                self,
                &mint_response.signatures,
                blinded_messages.iter(),
                SignatureAmountValidation::Exact,
            )
            .await?;

            let proofs = construct_proofs(
                mint_response.signatures,
                premint_secrets.rs(),
                premint_secrets.secrets(),
                &keys,
            )?;

            let proof_infos: Vec<ProofInfo> = proofs
                .into_iter()
                .map(|p| {
                    ProofInfo::new(p, self.mint_url.clone(), State::Unspent, self.unit.clone())
                })
                .collect::<Result<Vec<_>, _>>()?;

            return Ok(Some(proof_infos));
        }

        // Single quote replay (existing logic)
        // Get the mint quote to retrieve payment method and potentially sign the request
        let quote = match self
            .localstore
            .get_mint_quote(data.primary_quote_id())
            .await?
        {
            Some(q) => q,
            None => {
                tracing::debug!(
                    "Issue saga {} - mint quote not found, cannot replay",
                    saga_id
                );
                return Ok(None);
            }
        };

        // Construct the mint request
        let mut mint_request: MintRequest<String> = MintRequest {
            quote: data.primary_quote_id().to_string(),
            outputs: blinded_messages.clone(),
            signature: None,
        };

        // Sign the request if the quote has a signing key (required for bolt12)
        if let Some(secret_key) = self.mint_quote_signing_key(&quote).await? {
            if let Err(e) = mint_request.sign(&secret_key) {
                tracing::warn!(
                    "Issue saga {} - failed to sign mint request: {}, cannot replay",
                    saga_id,
                    e
                );
                return Ok(None);
            }
        }

        tracing::info!(
            "Issue saga {} - attempting replay of post_mint request",
            saga_id
        );

        let mint_request = PreparedMintRequest::Single {
            quote_id: data.primary_quote_id().to_string(),
            quote_info: quote.clone(),
            request: mint_request,
        };

        // Attempt the replay
        let mint_response = match super::post_mint_request_with_legacy_fallback(
            self,
            &quote.payment_method,
            &mint_request,
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                if is_mint_limit_error(&e) {
                    tracing::warn!(
                        "Issue saga {} - replay failed with mint limit: {}",
                        saga_id,
                        e
                    );
                    return Err(e);
                }

                tracing::info!(
                    "Issue saga {} - replay failed ({}), falling back to restore",
                    saga_id,
                    e
                );
                return Ok(None);
            }
        };

        // Replay succeeded - construct proofs from signatures
        tracing::info!(
            "Issue saga {} - replay succeeded, got {} signatures",
            saga_id,
            mint_response.signatures.len()
        );

        // We need to re-derive the secrets to unblind the signatures
        let (counter_start, counter_end) = match (data.counter_start, data.counter_end) {
            (Some(start), Some(end)) => (start, end),
            _ => {
                tracing::warn!(
                    "Issue saga {} - no counter range stored, cannot construct proofs",
                    saga_id
                );
                return Ok(None);
            }
        };

        let keyset_id = blinded_messages[0].keyset_id;

        let premint_secrets = crate::nuts::PreMintSecrets::restore_batch(
            keyset_id,
            &self.seed,
            counter_start,
            counter_end,
        )?;

        let keys = self.keyset(keyset_id).await?.keys;

        validate_mint_response_signatures(
            self,
            &mint_response.signatures,
            blinded_messages.iter(),
            SignatureAmountValidation::Exact,
        )
        .await?;

        let proofs = construct_proofs(
            mint_response.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        let proof_infos: Vec<ProofInfo> = proofs
            .into_iter()
            .map(|p| ProofInfo::new(p, self.mint_url.clone(), State::Unspent, self.unit.clone()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(proof_infos))
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
    use std::sync::Arc;

    use cdk_common::amount::{FeeAndAmounts, SplitTarget};
    use cdk_common::nuts::{CurrencyUnit, RestoreResponse};
    use cdk_common::wallet::{
        IssueSagaState, MintOperationData, OperationData, TransactionStatus, WalletSaga,
        WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::nuts::PreMintSecrets;
    use crate::wallet::recovery::RecoveryAction;
    use crate::wallet::saga::test_utils::{create_test_db, test_mint_url};
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, test_keyset_id, test_mint_quote, MockMintConnector,
    };

    #[test]
    fn test_only_mint_limit_errors_abort_replay_recovery() {
        assert!(super::is_mint_limit_error(
            &crate::Error::MaxOutputsExceeded { actual: 2, max: 1 }
        ));
        assert!(super::is_mint_limit_error(
            &crate::Error::MaxInputsExceeded { actual: 2, max: 1 }
        ));
        assert!(!super::is_mint_limit_error(&crate::Error::IssuedQuote));
        assert!(!super::is_mint_limit_error(&crate::Error::HttpError(
            Some(429),
            "Too Many Requests".to_string()
        )));
    }

    #[tokio::test]
    async fn test_recover_issue_secrets_prepared() {
        // Compensate: quote released
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_mint_quote_{}", uuid::Uuid::new_v4());

        // Store mint quote before reserving it
        let mut mint_quote = test_mint_quote(mint_url.clone());
        mint_quote.id = quote_id.clone(); // Use our specific quote ID
        db.add_mint_quote(mint_quote).await.unwrap();

        // Reserve mint quote
        db.reserve_mint_quote(&quote_id, &saga_id).await.unwrap();

        // Create saga in SecretsPrepared state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
            Amount::from(1000),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData::new_single(
                quote_id.clone(),
                Amount::from(1000),
                None,
                None,
                None,
            )),
        );
        db.add_saga(saga).await.unwrap();

        // Create wallet and recover
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Verify compensation
        assert!(result.is_ok());
        let recovery_action = result.unwrap();
        assert_eq!(recovery_action, RecoveryAction::Compensated);

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_issue_mint_requested_without_recovery_data_stays_pending() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_mint_quote_{}", uuid::Uuid::new_v4());

        // Create saga in MintRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Issue(IssueSagaState::MintRequested),
            Amount::from(1000),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData::new_single(
                quote_id.clone(),
                Amount::from(1000),
                Some(0),
                Some(10),
                Some(vec![]), // Empty for simplicity
            )),
        );
        db.add_saga(saga).await.unwrap();

        // Store mint quote
        let mint_quote = test_mint_quote(mint_url.clone());
        db.add_mint_quote(mint_quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_post_mint_response(Ok(crate::nuts::MintResponse { signatures: vec![] }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        assert!(result.is_ok());
        let recovery_action = result.unwrap();

        // Without stable output data, recovery cannot make a terminal decision.
        assert_eq!(recovery_action, RecoveryAction::Skipped);
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());

        // No proofs created
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        // No transaction can be reconstructed without stable output identifiers.
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert!(transactions.is_empty());
    }

    #[tokio::test]
    async fn test_recover_issue_empty_restore_marks_transaction_failed() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_mint_quote_{}", uuid::Uuid::new_v4());

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_post_mint_response(Err(crate::Error::Custom("Mint failed".to_string())));
        mock_client._set_restore_response(Ok(RestoreResponse {
            signatures: vec![],
            outputs: vec![],
        }));
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let fee_and_amounts = FeeAndAmounts::from((0, vec![1]));
        let premint_secrets = PreMintSecrets::from_seed(
            test_keyset_id(),
            0,
            &wallet.seed,
            Amount::from(1),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Issue(IssueSagaState::MintRequested),
            Amount::from(1),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData::new_single(
                quote_id.clone(),
                Amount::from(1),
                Some(0),
                Some(1),
                Some(premint_secrets.blinded_messages()),
            )),
        );
        db.add_saga(saga).await.unwrap();

        let mut mint_quote = test_mint_quote(mint_url);
        mint_quote.id = quote_id;
        mint_quote.used_by_operation = Some(saga_id.to_string());
        db.add_mint_quote(mint_quote).await.unwrap();

        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        assert!(result.is_ok());
        let recovery_action = result.unwrap();

        assert_eq!(recovery_action, RecoveryAction::Compensated);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // No proofs
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].status, TransactionStatus::Failed);
    }

    #[tokio::test]
    async fn test_recover_issue_mint_requested_max_outputs_does_not_restore() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();
        let quote_id = format!("test_mint_quote_{}", uuid::Uuid::new_v4());

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client
            .set_post_mint_response(Err(crate::Error::MaxOutputsExceeded { actual: 2, max: 1 }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let fee_and_amounts = FeeAndAmounts::from((0, vec![1]));
        let premint_secrets = PreMintSecrets::from_seed(
            test_keyset_id(),
            0,
            &wallet.seed,
            Amount::from(1),
            &SplitTarget::None,
            &fee_and_amounts,
        )
        .unwrap();

        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Issue(IssueSagaState::MintRequested),
            Amount::from(1),
            mint_url.clone(),
            CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData::new_single(
                quote_id.clone(),
                Amount::from(1),
                Some(0),
                Some(1),
                Some(premint_secrets.blinded_messages()),
            )),
        );
        db.add_saga(saga).await.unwrap();

        let mut mint_quote = test_mint_quote(mint_url);
        mint_quote.id = quote_id.clone();
        mint_quote.used_by_operation = Some(saga_id.to_string());
        db.add_mint_quote(mint_quote).await.unwrap();

        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        assert!(matches!(
            result,
            Err(crate::Error::MaxOutputsExceeded { actual: 2, max: 1 })
        ));
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        let mint_quote = db.get_mint_quote(&quote_id).await.unwrap().unwrap();
        assert!(mint_quote.used_by_operation.is_none());
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].status, TransactionStatus::Failed);
    }
}
