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
use tracing::instrument;

use crate::dhke::construct_proofs;
use crate::nuts::{MintRequest, State};
use crate::util::unix_time;
use crate::wallet::issue::saga::compensation::ReleaseMintQuote;
use crate::wallet::recovery::{RecoveryAction, RecoveryHelpers};
use crate::wallet::saga::CompensatingAction;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Resume an incomplete issue (mint) saga after crash recovery.
    ///
    /// # Recovery Logic
    ///
    /// - **SecretsPrepared**: Secrets created but mint request not sent.
    ///   Safe to compensate (no proofs to revert, just release quote and delete saga).
    ///
    /// - **MintRequested**: Mint request was sent. Try to recover outputs
    ///   using stored blinded messages.
    #[instrument(skip(self, saga))]
    pub async fn resume_issue_saga(&self, saga: &WalletSaga) -> Result<RecoveryAction, Error> {
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
    ///
    /// Uses a replay-first strategy:
    /// 1. Try to replay the original mint request (leverages NUT-19 caching)
    /// 2. If replay fails, fall back to /restore
    async fn complete_issue_from_restore(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<RecoveryAction, Error> {
        // Step 1: Try to replay the mint request
        if let Some(proofs) = self.try_replay_mint(saga_id, data).await? {
            // Replay succeeded - save proofs and clean up
            self.localstore
                .update_proofs(proofs.clone(), vec![])
                .await?;

            // Record transaction (best-effort, don't fail recovery if this fails)
            if let Err(e) = self
                .record_recovered_issue_transaction(saga_id, &data.quote_id, &proofs)
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

        // Step 2: Replay failed, fall back to /restore
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
                    .record_recovered_issue_transaction(saga_id, &data.quote_id, &proofs)
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
    ///
    /// This is called after successfully recovering proofs via replay or restore.
    /// If the quote is not found, the transaction is skipped (recovery still succeeds).
    async fn record_recovered_issue_transaction(
        &self,
        saga_id: &uuid::Uuid,
        quote_id: &str,
        proofs: &[ProofInfo],
    ) -> Result<(), Error> {
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
    /// Returns:
    /// - `Ok(Some(proofs))` if replay succeeded and we got signatures
    /// - `Ok(None)` if replay failed (fall back to /restore)
    /// - `Err` only for unrecoverable errors
    async fn try_replay_mint(
        &self,
        saga_id: &uuid::Uuid,
        data: &MintOperationData,
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
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

        // Get the mint quote to retrieve payment method and potentially sign the request
        let quote = match self.localstore.get_mint_quote(&data.quote_id).await? {
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
            quote: data.quote_id.clone(),
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
