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
    WalletSaga,
};
use cdk_common::{Amount, PaymentMethod};
use tracing::instrument;

use crate::dhke::construct_proofs;
use crate::nuts::{MintRequest, State};
use crate::util::unix_time;
use crate::wallet::issue::saga::compensation::ReleaseMintQuote;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::{Error, Wallet};

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

        // Try replay first
        if let Some(proofs) = self.try_replay_mint(saga_id, data).await? {
            // Replay succeeded - save proofs and clean up
            self.localstore
                .update_proofs(proofs.clone(), vec![])
                .await?;

            // Record transaction (best-effort, don't fail recovery if this fails)
            if let Err(e) = self
                .record_recovered_issue_transaction(saga_id, &quote_ids, &proofs)
                .await
            {
                tracing::warn!(
                    "Failed to record transaction for recovered issue saga {}: {}",
                    saga_id,
                    e
                );
            }

            self.localstore.delete_saga(saga_id).await?;
            return Ok(RecoveryAction::Recovered);
        }

        // Replay failed, fall back to /restore
        let new_proofs = self
            .restore_outputs(
                saga_id,
                "Issue",
                data.blinded_messages.as_deref(),
                data.counter_start,
                data.counter_end,
            )
            .await?;

        match new_proofs {
            Some(proofs) => {
                // Issue has no input proofs to remove - just add the recovered proofs
                self.localstore
                    .update_proofs(proofs.clone(), vec![])
                    .await?;

                // Record transaction (best-effort, don't fail recovery if this fails)
                if let Err(e) = self
                    .record_recovered_issue_transaction(saga_id, &quote_ids, &proofs)
                    .await
                {
                    tracing::warn!(
                        "Failed to record transaction for recovered issue saga {}: {}",
                        saga_id,
                        e
                    );
                }

                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Recovered)
            }
            None => {
                // Couldn't restore outputs - issue saga has no inputs to mark spent
                tracing::warn!(
                    "Issue saga {} - couldn't restore outputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_id
                );
                self.localstore.delete_saga(saga_id).await?;
                Ok(RecoveryAction::Compensated)
            }
        }
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

        self.localstore
            .add_transaction(Transaction {
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
                if let Some(secret_key) = &quote.secret_key {
                    let sig = batch_request
                        .sign_quote(&quote.id, secret_key)
                        .map_err(|e| Error::Custom(format!("NUT-20 signing failed: {}", e)))?;
                    signatures.push(Some(sig));
                } else {
                    signatures.push(None);
                }
            }

            let has_locked = quote_infos.iter().any(|q| q.secret_key.is_some());
            let signatures_to_send = if has_locked { Some(signatures) } else { None };
            batch_request.signatures = signatures_to_send;

            tracing::info!(
                "Issue saga {} - attempting replay of post_batch_mint request",
                saga_id
            );

            // Attempt batch replay
            let mint_response = match self
                .client
                .post_batch_mint(&payment_method, batch_request)
                .await
            {
                Ok(response) => response,
                Err(e) => {
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

            let keys = self.load_keyset_keys(keyset_id).await?;

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

        // Sign the request if the quote has a secret key (required for bolt12)
        if let Some(ref secret_key) = quote.secret_key {
            if let Err(e) = mint_request.sign(secret_key.clone()) {
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

        // Attempt the replay
        let mint_response = match self
            .client
            .post_mint(&quote.payment_method, mint_request)
            .await
        {
            Ok(response) => response,
            Err(e) => {
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

        let keys = self.load_keyset_keys(keyset_id).await?;

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

    use cdk_common::nuts::{CurrencyUnit, RestoreResponse};
    use cdk_common::wallet::{
        IssueSagaState, MintOperationData, OperationData, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::recovery::RecoveryAction;
    use crate::wallet::saga::test_utils::{create_test_db, test_mint_url};
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, test_mint_quote, MockMintConnector,
    };

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
    async fn test_recover_issue_mint_requested_replay_succeeds() {
        // Mock: post_mint succeeds → recovered
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

        // Mock: post_mint succeeds
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_post_mint_response(Ok(crate::nuts::MintResponse { signatures: vec![] }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Verify recovery
        assert!(result.is_ok());
        let recovery_action = result.unwrap();

        // With empty blinded_messages, falls back to restore
        // With empty restore response, saga deleted as Compensated
        assert_eq!(recovery_action, RecoveryAction::Compensated);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // No proofs created
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        // No transaction recorded for compensated issue
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert!(transactions.is_empty());
    }

    #[tokio::test]
    async fn test_recover_issue_mint_requested_restore_succeeds() {
        // Mock: post_mint fails, restore succeeds → recovered
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
                Some(vec![]),
            )),
        );
        db.add_saga(saga).await.unwrap();

        // Store mint quote
        let mint_quote = test_mint_quote(mint_url.clone());
        db.add_mint_quote(mint_quote).await.unwrap();

        // Mock: post_mint fails, restore returns proofs
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_post_mint_response(Err(crate::Error::Custom("Mint failed".to_string())));
        mock_client._set_restore_response(Ok(RestoreResponse {
            signatures: vec![],
            outputs: vec![],
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let result = wallet
            .resume_issue_saga(&db.get_saga(&saga_id).await.unwrap().unwrap())
            .await;

        // Verify recovery
        assert!(result.is_ok());
        let recovery_action = result.unwrap();

        // post_mint fails, restore returns empty -> Compensated
        assert_eq!(recovery_action, RecoveryAction::Compensated);
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());

        // No proofs
        let proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(proofs.is_empty());

        // No transaction for compensated issue
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert!(transactions.is_empty());
    }
}
