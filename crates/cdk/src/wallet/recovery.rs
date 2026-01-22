//! Wallet saga recovery module.
//!
//! This module handles recovery from incomplete wallet sagas after a crash.
//! It follows the same pattern as the mint's `start_up_check.rs` module.
//!
//! # Usage
//!
//! Call `recover_incomplete_sagas()` after creating the wallet and before
//! performing normal operations:
//!
//! ```rust,ignore
//! let wallet = WalletBuilder::new()
//!     .mint_url(mint_url)
//!     .unit(CurrencyUnit::Sat)
//!     .localstore(localstore)
//!     .seed(&seed)
//!     .build()?;
//!
//! // Recover from any incomplete operations from a previous crash
//! let report = wallet.recover_incomplete_sagas().await?;
//! if report.recovered > 0 || report.compensated > 0 {
//!     tracing::info!("Recovered {} operations, compensated {}",
//!         report.recovered, report.compensated);
//! }
//!
//! // Now safe to use the wallet normally
//! ```
//!
//! # Recovery Strategy
//!
//! For each incomplete saga, the recovery logic examines the saga state and takes
//! appropriate action:
//!
//! - **ProofsReserved**: No external call was made. Safe to compensate by releasing proofs.
//! - **SwapRequested**: External call may have succeeded. Check mint for proof states
//!   and either reconstruct outputs or compensate.

use async_trait::async_trait;
use cdk_common::wallet::WalletSagaState;
use cdk_common::BlindedMessage;
use tracing::instrument;

use crate::dhke::construct_proofs;
use crate::nuts::{CheckStateRequest, PreMintSecrets, Proofs, RestoreRequest, State, SwapRequest};
use crate::types::ProofInfo;
use crate::{Error, Wallet};

/// Parameters for recovering outputs using stored blinded messages.
///
/// This struct captures the common data needed across swap, receive, and issue
/// saga recovery operations.
struct OutputRecoveryParams<'a> {
    /// The blinded messages stored during the original operation
    blinded_messages: &'a [BlindedMessage],
    /// Counter start for re-deriving secrets
    counter_start: u32,
    /// Counter end for re-deriving secrets
    counter_end: u32,
}

/// Report of recovery operations performed
#[derive(Debug, Default)]
pub struct RecoveryReport {
    /// Number of sagas that were successfully recovered
    pub recovered: usize,
    /// Number of sagas that were compensated (rolled back)
    pub compensated: usize,
    /// Number of sagas that were skipped (e.g., pending external state)
    pub skipped: usize,
    /// Number of sagas that failed to recover
    pub failed: usize,
}

/// Result of a saga recovery operation.
///
/// Used by individual saga resume functions to indicate the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// The saga was successfully recovered (outputs saved)
    Recovered,
    /// The saga was compensated (rolled back)
    Compensated,
    /// The saga was skipped (e.g., mint unreachable, payment pending)
    Skipped,
}

/// Shared recovery helpers for saga resume operations.
///
/// These methods are used by individual saga resume modules to check
/// external state and restore outputs.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RecoveryHelpers {
    /// Check if all proofs are spent by querying the mint.
    ///
    /// This is a simple check that doesn't update the database.
    ///
    /// Returns:
    /// - `Ok(true)` if all proofs are spent
    /// - `Ok(false)` if any proofs are not spent
    /// - `Err` if the mint is unreachable
    async fn are_proofs_spent(&self, proofs: &[ProofInfo]) -> Result<bool, Error>;

    /// Restore outputs using stored blinded messages.
    ///
    /// Queries the mint's /restore endpoint to recover proof signatures,
    /// then reconstructs the proofs.
    ///
    /// Returns:
    /// - `Ok(Some(proofs))` if outputs were successfully restored
    /// - `Ok(None)` if restoration failed (mint unreachable, no data, etc.)
    /// - `Err` for unrecoverable errors (cryptographic failures, etc.)
    async fn restore_outputs(
        &self,
        saga_id: &uuid::Uuid,
        saga_type: &str,
        blinded_messages: Option<&[BlindedMessage]>,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
    ) -> Result<Option<Vec<ProofInfo>>, Error>;

    /// Attempt to replay a swap request using stored data.
    ///
    /// This leverages NUT-19 caching: if the mint has a cached response for the
    /// exact request, it will return the signatures immediately. This also works
    /// if the swap wasn't actually executed yet (inputs not spent).
    ///
    /// Works for both Swap and Receive sagas since both use `post_swap`.
    ///
    /// Returns:
    /// - `Ok(Some(proofs))` if replay succeeded and we got signatures
    /// - `Ok(None)` if replay failed (fall back to other recovery methods)
    /// - `Err` only for unrecoverable errors
    async fn try_replay_swap_request(
        &self,
        saga_id: &uuid::Uuid,
        saga_type: &str,
        blinded_messages: Option<&[BlindedMessage]>,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
        input_proofs: &[ProofInfo],
    ) -> Result<Option<Vec<ProofInfo>>, Error>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RecoveryHelpers for Wallet {
    /// Check if all proofs are spent by querying the mint.
    async fn are_proofs_spent(&self, proofs: &[ProofInfo]) -> Result<bool, Error> {
        if proofs.is_empty() {
            return Ok(false);
        }

        let ys: Vec<_> = proofs.iter().map(|p| p.y).collect();
        let response = self
            .client
            .post_check_state(CheckStateRequest { ys })
            .await?;

        Ok(response.states.iter().all(|s| s.state == State::Spent))
    }

    /// Restore outputs using stored blinded messages.
    async fn restore_outputs(
        &self,
        saga_id: &uuid::Uuid,
        saga_type: &str,
        blinded_messages: Option<&[BlindedMessage]>,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
        // Clone blinded messages to avoid temporary lifetime issues
        let blinded_messages_owned = blinded_messages.map(|bm| bm.to_vec());

        // Extract and validate parameters
        let params = match Self::extract_recovery_params(
            saga_id,
            saga_type,
            blinded_messages_owned.as_ref(),
            counter_start,
            counter_end,
        ) {
            Some(p) => p,
            None => return Ok(None),
        };

        self.recover_outputs_from_blinded_messages(saga_id, saga_type, params)
            .await
    }

    /// Attempt to replay a swap request using stored data.
    async fn try_replay_swap_request(
        &self,
        saga_id: &uuid::Uuid,
        saga_type: &str,
        blinded_messages: Option<&[BlindedMessage]>,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
        input_proofs: &[ProofInfo],
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
        // We need blinded messages to reconstruct the request
        let blinded_messages = match blinded_messages {
            Some(bm) if !bm.is_empty() => bm,
            _ => {
                tracing::debug!(
                    "{} saga {} - no blinded messages stored, cannot replay",
                    saga_type,
                    saga_id
                );
                return Ok(None);
            }
        };

        // We need counter range to re-derive secrets
        let (counter_start, counter_end) = match (counter_start, counter_end) {
            (Some(start), Some(end)) => (start, end),
            _ => {
                tracing::debug!(
                    "{} saga {} - no counter range stored, cannot replay",
                    saga_type,
                    saga_id
                );
                return Ok(None);
            }
        };

        // Extract input proofs
        let inputs: Proofs = input_proofs.iter().map(|pi| pi.proof.clone()).collect();

        if inputs.is_empty() {
            tracing::debug!(
                "{} saga {} - no input proofs available, cannot replay",
                saga_type,
                saga_id
            );
            return Ok(None);
        }

        // Reconstruct the swap request
        let swap_request = SwapRequest::new(inputs, blinded_messages.to_vec());

        tracing::info!(
            "{} saga {} - attempting replay of post_swap request",
            saga_type,
            saga_id
        );

        // Attempt the replay
        let swap_response = match self.client.post_swap(swap_request).await {
            Ok(response) => response,
            Err(e) => {
                tracing::info!(
                    "{} saga {} - replay failed ({}), falling back to other recovery",
                    saga_type,
                    saga_id,
                    e
                );
                return Ok(None);
            }
        };

        // Replay succeeded - construct proofs from signatures
        tracing::info!(
            "{} saga {} - replay succeeded, got {} signatures",
            saga_type,
            saga_id,
            swap_response.signatures.len()
        );

        // Get keyset ID from the first blinded message
        let keyset_id = blinded_messages[0].keyset_id;

        // Re-derive premint secrets
        let premint_secrets =
            PreMintSecrets::restore_batch(keyset_id, &self.seed, counter_start, counter_end)?;

        // Load keyset keys
        let keys = self.load_keyset_keys(keyset_id).await?;

        // Construct proofs
        let proofs = construct_proofs(
            swap_response.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        // Convert to ProofInfo
        let proof_infos: Vec<ProofInfo> = proofs
            .into_iter()
            .map(|p| ProofInfo::new(p, self.mint_url.clone(), State::Unspent, self.unit.clone()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(proof_infos))
    }
}

impl Wallet {
    /// Recover from incomplete sagas.
    ///
    /// This method should be called on wallet initialization to recover from
    /// any incomplete operations that were interrupted by a crash.
    ///
    /// # Returns
    ///
    /// A report of the recovery operations performed.
    #[instrument(skip(self))]
    pub async fn recover_incomplete_sagas(&self) -> Result<RecoveryReport, Error> {
        // First, clean up any orphaned quote reservations.
        // These can occur if the wallet crashed after reserving a quote
        // but before creating the saga record.
        self.cleanup_orphaned_quote_reservations().await?;

        let sagas = self.localstore.get_incomplete_sagas().await?;

        if sagas.is_empty() {
            tracing::debug!("No incomplete sagas to recover");
            return Ok(RecoveryReport::default());
        }

        tracing::info!("Found {} incomplete saga(s) to recover", sagas.len());

        let mut report = RecoveryReport::default();

        for saga in sagas {
            tracing::info!(
                "Recovering saga {} (kind: {:?}, state: {})",
                saga.id,
                saga.kind,
                saga.state.state_str()
            );

            // Delegate to the saga-specific resume functions
            let result: Result<RecoveryAction, Error> = match &saga.state {
                WalletSagaState::Swap(_) => self.resume_swap_saga(&saga).await,
                WalletSagaState::Send(_) => self.resume_send_saga(&saga).await,
                WalletSagaState::Receive(_) => self.resume_receive_saga(&saga).await,
                WalletSagaState::Issue(_) => self.resume_issue_saga(&saga).await,
                WalletSagaState::Melt(_) => {
                    // Melt saga returns Option<FinalizedMelt>, convert to RecoveryAction
                    self.resume_melt_saga(&saga).await.map(|opt| match opt {
                        Some(finalized) => {
                            use cdk_common::MeltQuoteState;
                            if finalized.state() == MeltQuoteState::Paid {
                                RecoveryAction::Recovered
                            } else {
                                RecoveryAction::Compensated
                            }
                        }
                        None => RecoveryAction::Skipped,
                    })
                }
            };

            match result {
                Ok(RecoveryAction::Recovered) => {
                    tracing::info!("Saga {} recovered successfully", saga.id);
                    report.recovered += 1;
                }
                Ok(RecoveryAction::Compensated) => {
                    tracing::info!("Saga {} compensated (rolled back)", saga.id);
                    report.compensated += 1;
                }
                Ok(RecoveryAction::Skipped) => {
                    tracing::info!("Saga {} skipped", saga.id);
                    report.skipped += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to recover saga {}: {}", saga.id, e);
                    report.failed += 1;
                    // Continue with other sagas - don't fail the entire recovery
                }
            }
        }

        tracing::info!(
            "Recovery complete: {} recovered, {} compensated, {} skipped, {} failed",
            report.recovered,
            report.compensated,
            report.skipped,
            report.failed
        );

        Ok(report)
    }

    /// Recover outputs using stored blinded messages.
    ///
    /// This is the core recovery logic shared between swap, receive, and issue saga
    /// recovery. It queries the mint for signatures using the stored blinded messages
    /// and reconstructs the proofs.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(proofs))` - Successfully recovered proofs
    /// - `Ok(None)` - Could not recover (mint unreachable, no signatures, etc.)
    ///   Caller should fall back to cleanup.
    /// - `Err(_)` - Unrecoverable error (e.g., cryptographic failure)
    #[instrument(skip(self, params))]
    async fn recover_outputs_from_blinded_messages(
        &self,
        saga_id: &uuid::Uuid,
        saga_type: &str,
        params: OutputRecoveryParams<'_>,
    ) -> Result<Option<Vec<ProofInfo>>, Error> {
        tracing::info!(
            "{} saga {} - attempting to recover {} outputs using stored blinded messages",
            saga_type,
            saga_id,
            params.blinded_messages.len()
        );

        // Query the mint for signatures using the stored blinded messages
        let restore_request = RestoreRequest {
            outputs: params.blinded_messages.to_vec(),
        };

        let restore_response = match self.client.post_restore(restore_request).await {
            Ok(response) => response,
            Err(e) => {
                if e.is_definitive_failure() {
                    tracing::warn!(
                        "{} saga {} - failed to restore from mint (definitive): {}. \
                         Run wallet.restore() to recover any missing proofs.",
                        saga_type,
                        saga_id,
                        e
                    );
                    return Ok(None);
                } else {
                    tracing::warn!(
                        "{} saga {} - failed to restore from mint (ambiguous): {}. \
                         Skipping recovery to retry later.",
                        saga_type,
                        saga_id,
                        e
                    );
                    return Err(e);
                }
            }
        };

        if restore_response.signatures.is_empty() {
            tracing::warn!(
                "{} saga {} - mint returned no signatures. \
                 Outputs may have already been saved or mint doesn't have them.",
                saga_type,
                saga_id
            );
            return Ok(None);
        }

        // Get keyset ID from the first blinded message
        let keyset_id = params.blinded_messages[0].keyset_id;

        // Re-derive premint secrets using the counter range
        let premint_secrets = PreMintSecrets::restore_batch(
            keyset_id,
            &self.seed,
            params.counter_start,
            params.counter_end,
        )?;

        // Match the returned outputs to our premint secrets by B_ value
        let matched_secrets: Vec<_> = premint_secrets
            .secrets
            .iter()
            .filter(|p| restore_response.outputs.contains(&p.blinded_message))
            .collect();

        if matched_secrets.len() != restore_response.signatures.len() {
            tracing::warn!(
                "{} saga {} - signature count mismatch: {} secrets, {} signatures",
                saga_type,
                saga_id,
                matched_secrets.len(),
                restore_response.signatures.len()
            );
        }

        // Load keyset keys for proof construction
        let keys = self.load_keyset_keys(keyset_id).await?;

        // Construct proofs from signatures
        let proofs = construct_proofs(
            restore_response.signatures,
            matched_secrets.iter().map(|p| p.r.clone()).collect(),
            matched_secrets.iter().map(|p| p.secret.clone()).collect(),
            &keys,
        )?;

        tracing::info!(
            "{} saga {} - recovered {} proofs",
            saga_type,
            saga_id,
            proofs.len()
        );

        // Convert to ProofInfo
        let proof_infos: Vec<ProofInfo> = proofs
            .into_iter()
            .map(|p| ProofInfo::new(p, self.mint_url.clone(), State::Unspent, self.unit.clone()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(proof_infos))
    }

    /// Extract recovery parameters from operation data.
    ///
    /// Returns `None` if the required data (blinded messages, counter range) is missing.
    fn extract_recovery_params<'a>(
        saga_id: &uuid::Uuid,
        saga_type: &str,
        blinded_messages: Option<&'a Vec<BlindedMessage>>,
        counter_start: Option<u32>,
        counter_end: Option<u32>,
    ) -> Option<OutputRecoveryParams<'a>> {
        let blinded_messages = match blinded_messages {
            Some(bm) if !bm.is_empty() => bm,
            _ => {
                tracing::warn!(
                    "{} saga {} - no blinded messages stored, cannot recover outputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_type,
                    saga_id
                );
                return None;
            }
        };

        let (counter_start, counter_end) = match (counter_start, counter_end) {
            (Some(start), Some(end)) => (start, end),
            _ => {
                tracing::warn!(
                    "{} saga {} - no counter range stored, cannot recover outputs. \
                     Run wallet.restore() to recover any missing proofs.",
                    saga_type,
                    saga_id
                );
                return None;
            }
        };

        Some(OutputRecoveryParams {
            blinded_messages,
            counter_start,
            counter_end,
        })
    }

    /// Clean up orphaned quote reservations.
    ///
    /// This handles the case where the wallet crashed after reserving a quote
    /// but before creating the saga record. In this case, the quote is stuck
    /// in a reserved state with no corresponding saga.
    ///
    /// This method:
    /// 1. Gets all quotes with `used_by_operation` set
    /// 2. Checks if a corresponding saga exists
    /// 3. If no saga exists, releases the quote reservation
    #[instrument(skip(self))]
    async fn cleanup_orphaned_quote_reservations(&self) -> Result<(), Error> {
        // Check melt quotes for orphaned reservations
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        for quote in melt_quotes {
            if let Some(ref operation_id_str) = quote.used_by_operation {
                if let Ok(operation_id) = uuid::Uuid::parse_str(operation_id_str) {
                    // Check if saga exists
                    match self.localstore.get_saga(&operation_id).await {
                        Ok(Some(_)) => {
                            // Saga exists, this is not orphaned
                        }
                        Ok(None) => {
                            // No saga found - this is an orphaned reservation
                            tracing::warn!(
                                "Found orphaned melt quote reservation: quote={}, operation={}. Releasing.",
                                quote.id,
                                operation_id
                            );
                            if let Err(e) = self.localstore.release_melt_quote(&operation_id).await
                            {
                                tracing::error!(
                                    "Failed to release orphaned melt quote {}: {}",
                                    quote.id,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to check saga for melt quote {}: {}",
                                quote.id,
                                e
                            );
                        }
                    }
                }
            }
        }

        // Check mint quotes for orphaned reservations
        let mint_quotes = self.localstore.get_mint_quotes().await?;
        for quote in mint_quotes {
            if let Some(ref operation_id_str) = quote.used_by_operation {
                if let Ok(operation_id) = uuid::Uuid::parse_str(operation_id_str) {
                    // Check if saga exists
                    match self.localstore.get_saga(&operation_id).await {
                        Ok(Some(_)) => {
                            // Saga exists, this is not orphaned
                        }
                        Ok(None) => {
                            // No saga found - this is an orphaned reservation
                            tracing::warn!(
                                "Found orphaned mint quote reservation: quote={}, operation={}. Releasing.",
                                quote.id,
                                operation_id
                            );
                            if let Err(e) = self.localstore.release_mint_quote(&operation_id).await
                            {
                                tracing::error!(
                                    "Failed to release orphaned mint quote {}: {}",
                                    quote.id,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to check saga for mint quote {}: {}",
                                quote.id,
                                e
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::nuts::{MeltQuoteBolt11Response, MeltQuoteState, State};
    use cdk_common::wallet::{
        IssueSagaState, MeltOperationData, MeltSagaState, MintOperationData, OperationData,
        ReceiveOperationData, ReceiveSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use crate::wallet::test_utils::*;

    #[tokio::test]
    async fn test_recover_receive_proofs_pending() {
        // Test that receive saga in ProofsPending state gets compensated:
        // - Saga is deleted
        // - If there are no reserved proofs (proofs are just Pending), just cleanup saga
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();

        // Create saga in ProofsPending state (no proofs to reserve for this test,
        // just test that the saga gets cleaned up)
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Receive(ReceiveSagaState::ProofsPending),
            Amount::from(100),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                token: "cashu...".to_string(),
                counter_start: None,
                counter_end: None,
                amount: Some(Amount::from(100)),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.compensated, 1);

        // Verify saga is deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_issue_secrets_prepared() {
        // Test that issue saga in SecretsPrepared state gets compensated:
        // - Quote reservation is released
        // - Saga is deleted
        // - Counter gaps are acceptable (not tested here, just cleanup)
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let saga_id = uuid::Uuid::new_v4();

        // Create a mint quote reserved by this operation
        let mut quote = test_mint_quote(mint_url.clone());
        quote.used_by_operation = Some(saga_id.to_string());
        db.add_mint_quote(quote.clone()).await.unwrap();

        // Create saga in SecretsPrepared state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
            Amount::from(1000),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Mint(MintOperationData {
                quote_id: quote.id.clone(),
                amount: Amount::from(1000),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.compensated, 1);

        // Verify quote reservation is released
        let retrieved_quote = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());

        // Verify saga is deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_proofs_reserved() {
        // Test that melt saga in ProofsReserved state gets compensated:
        // - Reserved proofs are released
        // - Quote reservation is released
        // - Saga is deleted
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and store proofs, then reserve them
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create a melt quote reserved by this operation
        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(saga_id.to_string());
        db.add_melt_quote(quote.clone()).await.unwrap();

        // Create saga in ProofsReserved state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            Amount::from(100),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote.id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
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

        // Verify melt quote reservation is released
        let retrieved_quote = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());

        // Verify saga is deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_no_incomplete_sagas() {
        // Test that recovery with no incomplete sagas returns empty report
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;

        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.recovered, 0);
        assert_eq!(report.compensated, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);
    }

    #[tokio::test]
    async fn test_recover_multiple_sagas() {
        // Test that recovery handles multiple sagas
        // (Note: We'll use Melt sagas here since we removed the Swap logic for testing)
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create 3 sagas in different early states
        for i in 0..3 {
            let saga_id = uuid::Uuid::new_v4();

            let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
            let proof_y = proof_info.y;
            db.update_proofs(vec![proof_info], vec![]).await.unwrap();
            db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

            let mut quote = test_melt_quote();
            quote.id = format!("quote_{}", i);
            quote.used_by_operation = Some(saga_id.to_string());
            db.add_melt_quote(quote.clone()).await.unwrap();

            let saga = WalletSaga::new(
                saga_id,
                WalletSagaState::Melt(MeltSagaState::ProofsReserved),
                Amount::from(100),
                mint_url.clone(),
                cdk_common::nuts::CurrencyUnit::Sat,
                OperationData::Melt(MeltOperationData {
                    quote_id: quote.id.clone(),
                    amount: Amount::from(100),
                    fee_reserve: Amount::from(10),
                    counter_start: None,
                    counter_end: None,
                    change_amount: None,
                    change_blinded_messages: None,
                }),
            );
            db.add_saga(saga).await.unwrap();
        }

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        assert_eq!(report.compensated, 3);

        // All proofs should be released
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 3);

        // All sagas should be deleted
        let sagas = db.get_incomplete_sagas().await.unwrap();
        assert!(sagas.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_melt_quote_reservation() {
        // Test that orphaned melt quote reservations are cleaned up
        let db = create_test_db().await;
        let operation_id = uuid::Uuid::new_v4();

        // Create a melt quote with reservation but NO corresponding saga
        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(operation_id.to_string());
        db.add_melt_quote(quote.clone()).await.unwrap();

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let _report = wallet.recover_incomplete_sagas().await.unwrap();

        // Verify orphaned quote reservation is released
        let retrieved_quote = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_orphaned_mint_quote_reservation() {
        // Test that orphaned mint quote reservations are cleaned up
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let operation_id = uuid::Uuid::new_v4();

        // Create a mint quote with reservation but NO corresponding saga
        let mut quote = test_mint_quote(mint_url);
        quote.used_by_operation = Some(operation_id.to_string());
        db.add_mint_quote(quote.clone()).await.unwrap();

        // Run recovery
        let wallet = create_test_wallet(db.clone()).await;
        let _report = wallet.recover_incomplete_sagas().await.unwrap();

        // Verify orphaned quote reservation is released
        let retrieved_quote = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_requested_quote_failed() {
        // When melt quote failed, should compensate
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create a melt quote
        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(saga_id.to_string());
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: quote is Failed
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            amount: Amount::from(100),
            fee_reserve: Amount::from(10),
            state: MeltQuoteState::Failed,
            expiry: 9999999999,
            payment_preimage: None,
            change: None,
            request: None,
            unit: None,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Should compensate (quote failed)
        assert_eq!(report.compensated, 1);
        assert_eq!(report.recovered, 0);

        // Proofs should be released
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Melt quote reservation should be released
        let retrieved_quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_recover_melt_requested_quote_pending() {
        // When melt quote payment is still in progress (Pending), should skip
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create a melt quote
        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(saga_id.to_string());
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: quote payment is in progress (Pending state)
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            amount: Amount::from(100),
            fee_reserve: Amount::from(10),
            state: MeltQuoteState::Pending,
            expiry: 9999999999,
            payment_preimage: None,
            change: None,
            request: None,
            unit: None,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Should skip (payment still in progress)
        assert_eq!(report.skipped, 1);
        assert_eq!(report.compensated, 0);
        assert_eq!(report.recovered, 0);

        // Proofs should still be reserved
        let reserved = db.get_reserved_proofs(&saga_id).await.unwrap();
        assert_eq!(reserved.len(), 1);

        // Saga should still exist
        assert!(db.get_saga(&saga_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_recover_melt_requested_quote_unpaid() {
        // When melt quote is Unpaid (payment was never initiated), should compensate
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let saga_id = uuid::Uuid::new_v4();

        // Create and reserve proofs
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone());
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();
        db.reserve_proofs(vec![proof_y], &saga_id).await.unwrap();

        // Create a melt quote
        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(saga_id.to_string());
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        // Create saga in MeltRequested state
        let saga = WalletSaga::new(
            saga_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            Amount::from(100),
            mint_url.clone(),
            cdk_common::nuts::CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: Amount::from(100),
                fee_reserve: Amount::from(10),
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        // Mock: quote is Unpaid (payment was never initiated or was rolled back)
        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            quote: quote_id.clone(),
            amount: Amount::from(100),
            fee_reserve: Amount::from(10),
            state: MeltQuoteState::Unpaid,
            expiry: 9999999999,
            payment_preimage: None,
            change: None,
            request: None,
            unit: None,
        }));

        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let report = wallet.recover_incomplete_sagas().await.unwrap();

        // Should compensate (payment was never initiated, safe to release proofs)
        assert_eq!(report.compensated, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.recovered, 0);

        // Proofs should be released back to Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);

        // Melt quote reservation should be released
        let retrieved_quote = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());

        // Saga should be deleted
        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }
}
