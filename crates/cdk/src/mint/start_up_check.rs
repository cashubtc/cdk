//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use std::str::FromStr;

use cdk_common::database::Error as DatabaseError;
use cdk_common::mint::OperationKind;
use cdk_common::QuoteId;

use super::{Error, Mint};
use crate::mint::swap::swap_saga::compensation::{CompensatingAction, RemoveSwapSetup};
use crate::mint::{MeltQuote, MeltQuoteState};
use crate::types::PaymentProcessorKey;

impl Mint {
    /// Checks the payment status of a melt quote with the LN backend
    ///
    /// This is a helper function used by saga recovery to determine whether to
    /// finalize or compensate an incomplete melt operation.
    ///
    /// # Returns
    ///
    /// - `Ok(MakePaymentResponse)`: Payment status successfully retrieved from backend
    /// - `Err(Error)`: Failed to check payment status (backend unavailable, no lookup_id, etc.)
    async fn check_melt_payment_status(
        &self,
        quote: &MeltQuote,
    ) -> Result<crate::cdk_payment::MakePaymentResponse, Error> {
        let ln_key = PaymentProcessorKey {
            unit: quote.unit.clone(),
            method: quote.payment_method.clone(),
        };

        let ln_backend = self.payment_processors.get(&ln_key).ok_or_else(|| {
            tracing::warn!("No backend for ln key: {:?}", ln_key);
            Error::UnsupportedUnit
        })?;

        let lookup_id = quote.request_lookup_id.as_ref().ok_or_else(|| {
            tracing::warn!(
                "No lookup_id for melt quote {}, cannot check payment status",
                quote.id
            );
            Error::Internal
        })?;

        // Check payment status with LN backend
        let pay_invoice_response =
            ln_backend
                .check_outgoing_payment(lookup_id)
                .await
                .map_err(|err| {
                    tracing::error!(
                        "Failed to check payment status for quote {}: {}",
                        quote.id,
                        err
                    );
                    Error::Internal
                })?;

        tracing::info!(
            "Payment status for melt quote {}: {}",
            quote.id,
            pay_invoice_response.status
        );

        Ok(pay_invoice_response)
    }

    /// Finalizes a paid melt quote during startup check
    ///
    /// Uses shared finalization logic from melt::shared module
    async fn finalize_paid_melt_quote(
        &self,
        quote: &MeltQuote,
        total_spent: cdk_common::Amount,
        payment_preimage: Option<String>,
        payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    ) -> Result<(), Error> {
        tracing::info!("Finalizing paid melt quote {} during startup", quote.id);

        // Use shared finalization
        super::melt::shared::finalize_melt_quote(
            self,
            &self.localstore,
            &self.pubsub_manager,
            quote,
            total_spent,
            payment_preimage,
            payment_lookup_id,
        )
        .await?;

        tracing::info!(
            "Successfully finalized melt quote {} during startup check",
            quote.id
        );

        Ok(())
    }

    /// Checks all persisted sagas for swap operations and compensates
    /// incomplete ones by removing both proofs and blinded messages.
    pub async fn recover_from_incomplete_sagas(&self) -> Result<(), Error> {
        let incomplete_sagas = self
            .localstore
            .get_incomplete_sagas(OperationKind::Swap)
            .await?;

        if incomplete_sagas.is_empty() {
            tracing::info!("No incomplete swap sagas found to recover.");
            return Ok(());
        }

        let total_sagas = incomplete_sagas.len();
        tracing::info!("Found {} incomplete swap sagas to recover.", total_sagas);

        for saga in incomplete_sagas {
            tracing::info!(
                "Recovering saga {} in state '{}' (created: {}, updated: {})",
                saga.operation_id,
                saga.state.state(),
                saga.created_at,
                saga.updated_at
            );

            // Look up input_ys and blinded_secrets from the proof and blind_signature tables
            let input_ys = self
                .localstore
                .get_proof_ys_by_operation_id(&saga.operation_id)
                .await?;
            let blinded_secrets = self
                .localstore
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;

            // Use the same compensation logic as in-process failures
            // Saga deletion is included in the compensation transaction
            let compensation = RemoveSwapSetup {
                blinded_secrets,
                input_ys,
                operation_id: saga.operation_id,
            };

            // Execute compensation (includes saga deletion)
            if let Err(e) = compensation.execute(&self.localstore).await {
                tracing::error!(
                    "Failed to compensate saga {}: {}. Continuing...",
                    saga.operation_id,
                    e
                );
                continue;
            }

            tracing::info!("Successfully recovered saga {}", saga.operation_id);
        }

        tracing::info!(
            "Successfully recovered {} incomplete swap sagas.",
            total_sagas
        );

        Ok(())
    }

    /// Recover from incomplete melt sagas
    ///
    /// Checks all persisted sagas for melt operations and determines whether to:
    /// - **Finalize**: If payment was confirmed as PAID on LN backend
    /// - **Compensate**: If payment was confirmed as UNPAID/FAILED or never sent
    /// - **Skip**: If payment is PENDING/UNKNOWN (leave for check_pending_melt_quotes)
    ///
    /// This recovery handles SetupComplete state which means:
    /// - Proofs were reserved (marked as PENDING)
    /// - Change outputs were added
    /// - Payment may or may not have been sent
    ///
    /// # Critical Bug Fix
    ///
    /// Previously, this function always compensated (rolled back) incomplete sagas without
    /// checking if the payment actually succeeded on the LN backend. This could cause the
    /// mint to lose funds if:
    /// 1. Payment succeeded on LN backend
    /// 2. Mint crashed before finalize() committed
    /// 3. Recovery compensated (returned proofs) instead of finalizing
    ///
    /// Now we check the LN backend payment status before deciding whether to compensate or finalize.
    pub async fn recover_from_incomplete_melt_sagas(&self) -> Result<(), Error> {
        let incomplete_sagas = self
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await?;

        if incomplete_sagas.is_empty() {
            tracing::info!("No incomplete melt sagas found to recover.");
            return Ok(());
        }

        let total_sagas = incomplete_sagas.len();
        tracing::info!("Found {} incomplete melt sagas to recover.", total_sagas);

        for saga in incomplete_sagas {
            tracing::info!(
                "Recovering melt saga {} in state '{}' (created: {}, updated: {})",
                saga.operation_id,
                saga.state.state(),
                saga.created_at,
                saga.updated_at
            );

            // Look up input_ys and blinded_secrets from the proof and blind_signature tables
            let input_ys = self
                .localstore
                .get_proof_ys_by_operation_id(&saga.operation_id)
                .await?;
            let blinded_secrets = self
                .localstore
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;

            // Get quote_id from saga (new field added for efficient lookup)
            let quote_id = match saga.quote_id {
                Some(ref qid) => qid.clone(),
                None => {
                    tracing::warn!(
                        "Saga {} has no quote_id (old saga format) - attempting fallback lookup",
                        saga.operation_id
                    );

                    // Fallback: Find quote by matching input_ys (for backward compatibility)
                    let melt_quotes = match self.localstore.get_melt_quotes().await {
                        Ok(quotes) => quotes,
                        Err(e) => {
                            tracing::error!(
                                "Failed to get melt quotes for saga {}: {}",
                                saga.operation_id,
                                e
                            );
                            continue;
                        }
                    };

                    let mut quote_id_found = None;
                    for quote in melt_quotes {
                        let mut tx = self.localstore.begin_transaction().await?;
                        let proof_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;
                        tx.rollback().await?;

                        if !input_ys.is_empty()
                            && !proof_ys.is_empty()
                            && input_ys.iter().any(|y| proof_ys.contains(y))
                        {
                            quote_id_found = Some(quote.id.clone());
                            break;
                        }
                    }

                    match quote_id_found {
                        Some(qid) => qid.to_string(),
                        None => {
                            tracing::warn!(
                                "Could not find quote_id for saga {} - may have been cleaned up already. Deleting orphaned saga.",
                                saga.operation_id
                            );

                            let mut delete_tx = self.localstore.begin_transaction().await?;
                            if let Err(e) = delete_tx.delete_saga(&saga.operation_id).await {
                                tracing::error!(
                                    "Failed to delete orphaned saga {}: {}",
                                    saga.operation_id,
                                    e
                                );
                                delete_tx.rollback().await?;
                            } else {
                                delete_tx.commit().await?;
                            }
                            continue;
                        }
                    }
                }
            };

            // Get the quote from database
            let quote_id_parsed = match QuoteId::from_str(&quote_id) {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!(
                        "Failed to parse quote_id '{}' for saga {}: {:?}. Skipping saga.",
                        quote_id,
                        saga.operation_id,
                        e
                    );
                    continue;
                }
            };

            let quote = match self.localstore.get_melt_quote(&quote_id_parsed).await {
                Ok(Some(q)) => q,
                Ok(None) => {
                    tracing::warn!(
                        "Quote {} for saga {} not found - may have been cleaned up. Deleting orphaned saga.",
                        quote_id,
                        saga.operation_id
                    );

                    let mut delete_tx = self.localstore.begin_transaction().await?;
                    if let Err(e) = delete_tx.delete_saga(&saga.operation_id).await {
                        tracing::error!(
                            "Failed to delete orphaned saga {}: {}",
                            saga.operation_id,
                            e
                        );
                        delete_tx.rollback().await?;
                    } else {
                        delete_tx.commit().await?;
                    }
                    continue;
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get quote {} for saga {}: {}. Skipping saga.",
                        quote_id,
                        saga.operation_id,
                        e
                    );
                    continue;
                }
            };

            // Check saga state to determine if payment was attempted
            // SetupComplete means setup transaction committed but payment NOT yet attempted
            // PaymentAttempted means payment was attempted - must check LN backend
            let should_compensate = match &saga.state {
                cdk_common::mint::SagaStateEnum::Melt(state) => {
                    match state {
                        cdk_common::mint::MeltSagaState::SetupComplete => {
                            // Setup complete but payment never attempted - always compensate
                            tracing::info!(
                                "Saga {} in SetupComplete state - payment never attempted, will compensate",
                                saga.operation_id
                            );
                            true
                        }
                        cdk_common::mint::MeltSagaState::PaymentAttempted => {
                            // Payment was attempted - check for internal settlement first, then LN backend
                            tracing::info!(
                                "Saga {} in PaymentAttempted state - checking for internal or external payment",
                                saga.operation_id
                            );

                            // Check if this was an internal settlement by looking for a mint quote
                            // that was paid by this melt quote
                            let is_internal_settlement = match self
                                .localstore
                                .get_mint_quote_by_request(&quote.request.to_string())
                                .await
                            {
                                Ok(Some(mint_quote)) => {
                                    // Check if this mint quote was paid by our melt quote
                                    let melt_quote_id_str = quote.id.to_string();
                                    mint_quote.payment_ids().contains(&&melt_quote_id_str)
                                }
                                Ok(None) => false,
                                Err(e) => {
                                    tracing::warn!(
                                        "Error checking for internal settlement for saga {}: {}",
                                        saga.operation_id,
                                        e
                                    );
                                    false
                                }
                            };

                            if is_internal_settlement {
                                // Internal settlement was completed - finalize directly
                                tracing::info!(
                                    "Saga {} was internal settlement - will finalize directly",
                                    saga.operation_id
                                );

                                // Get payment info for finalization
                                let total_spent = quote.amount;
                                let payment_lookup_id =
                                    quote.request_lookup_id.clone().unwrap_or_else(|| {
                                        cdk_common::payment::PaymentIdentifier::CustomId(
                                            quote.id.to_string(),
                                        )
                                    });

                                if let Err(err) = self
                                    .finalize_paid_melt_quote(
                                        &quote,
                                        total_spent,
                                        None, // No preimage for internal settlement
                                        &payment_lookup_id,
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to finalize internal settlement saga {}: {}",
                                        saga.operation_id,
                                        err
                                    );
                                }

                                // Delete saga after successful finalization
                                let mut tx = self.localstore.begin_transaction().await?;
                                if let Err(e) = tx.delete_saga(&saga.operation_id).await {
                                    tracing::error!(
                                        "Failed to delete saga for {}: {}",
                                        saga.operation_id,
                                        e
                                    );
                                    tx.rollback().await?;
                                } else {
                                    tx.commit().await?;
                                    tracing::info!(
                                        "Successfully recovered and finalized internal settlement saga {}",
                                        saga.operation_id
                                    );
                                }

                                continue; // Skip to next saga
                            }

                            false // Will check LN payment status below
                        }
                    }
                }
                _ => {
                    continue; // Skip non-melt sagas
                }
            };

            let should_compensate = if should_compensate {
                true
            } else if quote.request_lookup_id.is_none() {
                // Fallback: No request_lookup_id means payment likely never sent
                tracing::info!(
                    "Saga {} for quote {} has no request_lookup_id - payment never sent, will compensate",
                    saga.operation_id,
                    quote_id
                );
                true
            } else {
                // Payment was attempted - check LN backend status
                tracing::info!(
                    "Saga {} for quote {} has request_lookup_id - checking payment status with LN backend",
                    saga.operation_id,
                    quote_id
                );

                match self.check_melt_payment_status(&quote).await {
                    Ok(payment_response) => {
                        match payment_response.status {
                            MeltQuoteState::Paid => {
                                // Payment succeeded - finalize instead of compensating
                                tracing::info!(
                                    "Saga {} for quote {} - payment PAID on LN backend, will finalize",
                                    saga.operation_id,
                                    quote_id
                                );

                                if let Err(err) = self
                                    .finalize_paid_melt_quote(
                                        &quote,
                                        payment_response.total_spent,
                                        payment_response.payment_proof,
                                        &payment_response.payment_lookup_id,
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to finalize paid melt saga {}: {}",
                                        saga.operation_id,
                                        err
                                    );
                                }

                                // Delete saga after successful finalization
                                let mut tx = self.localstore.begin_transaction().await?;
                                if let Err(e) = tx.delete_saga(&saga.operation_id).await {
                                    tracing::error!(
                                        "Failed to delete saga for {}: {}",
                                        saga.operation_id,
                                        e
                                    );
                                    tx.rollback().await?;
                                } else {
                                    tx.commit().await?;
                                    tracing::info!(
                                        "Successfully recovered and finalized melt saga {}",
                                        saga.operation_id
                                    );
                                }

                                continue; // Skip compensation, saga handled
                            }
                            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                                // Payment failed - compensate
                                tracing::info!(
                                    "Saga {} for quote {} - payment {} on LN backend, will compensate",
                                    saga.operation_id,
                                    quote_id,
                                    payment_response.status
                                );
                                true
                            }
                            MeltQuoteState::Pending | MeltQuoteState::Unknown => {
                                // Payment still pending - skip for check_pending_melt_quotes
                                tracing::info!(
                                    "Saga {} for quote {} - payment {} on LN backend, skipping (will be handled by check_pending_melt_quotes)",
                                    saga.operation_id,
                                    quote_id,
                                    payment_response.status
                                );
                                continue; // Skip this saga, don't compensate or finalize
                            }
                        }
                    }
                    Err(err) => {
                        // LN backend unavailable - skip this saga, will retry on next recovery cycle
                        tracing::warn!(
                            "Failed to check payment status for saga {} quote {}: {}. Skipping for now, will retry on next recovery cycle.",
                            saga.operation_id,
                            quote_id,
                            err
                        );
                        continue; // Skip this saga
                    }
                }
            };

            // Compensate if needed
            if should_compensate {
                tracing::info!(
                    "Compensating melt saga {} (removing {} proofs, {} change outputs)",
                    saga.operation_id,
                    input_ys.len(),
                    blinded_secrets.len()
                );

                let mut tx = self.localstore.begin_transaction().await?;

                // Remove blinded messages (change outputs)
                if !blinded_secrets.is_empty() {
                    if let Err(e) = tx.delete_blinded_messages(&blinded_secrets).await {
                        tracing::error!(
                            "Failed to delete blinded messages for saga {}: {}",
                            saga.operation_id,
                            e
                        );
                        tx.rollback().await?;
                        continue;
                    }
                }

                // Remove proofs (inputs)
                if !input_ys.is_empty() {
                    match tx.remove_proofs(&input_ys, None).await {
                        Ok(()) => {}
                        Err(DatabaseError::AttemptRemoveSpentProof) => {
                            // Proofs are already spent or missing - this is okay for compensation.
                            // The goal is to make proofs unusable, and they already are.
                            // Continue with saga deletion to avoid infinite recovery loop.
                            tracing::warn!(
                                "Saga {} compensation: proofs already spent or missing, proceeding with saga cleanup",
                                saga.operation_id
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to remove proofs for saga {}: {}",
                                saga.operation_id,
                                e
                            );
                            tx.rollback().await?;
                            continue;
                        }
                    }
                }

                // Reset quote state to Unpaid (melt-specific, unlike swap)
                // Acquire lock on the quote first
                let mut locked_quote = match tx.get_melt_quote(&quote_id_parsed).await {
                    Ok(Some(q)) => q,
                    Ok(None) => {
                        tracing::warn!(
                            "Melt quote {} not found for saga {} - may have been cleaned up",
                            quote_id_parsed,
                            saga.operation_id
                        );
                        // Continue with saga deletion even if quote is gone
                        if let Err(e) = tx.delete_saga(&saga.operation_id).await {
                            tracing::error!("Failed to delete saga {}: {}", saga.operation_id, e);
                            tx.rollback().await?;
                            continue;
                        }
                        tx.commit().await?;
                        continue;
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to get quote for saga {}: {}",
                            saga.operation_id,
                            e
                        );
                        tx.rollback().await?;
                        continue;
                    }
                };

                if let Err(e) = tx
                    .update_melt_quote_state(&mut locked_quote, MeltQuoteState::Unpaid, None)
                    .await
                {
                    tracing::error!(
                        "Failed to reset quote state for saga {}: {}",
                        saga.operation_id,
                        e
                    );
                    tx.rollback().await?;
                    continue;
                }

                // Delete melt request tracking record
                if let Err(e) = tx.delete_melt_request(&quote_id_parsed).await {
                    tracing::error!(
                        "Failed to delete melt request for saga {}: {}",
                        saga.operation_id,
                        e
                    );
                    // Don't fail if melt request doesn't exist - it might not have been created yet
                }

                // Delete saga after successful compensation
                if let Err(e) = tx.delete_saga(&saga.operation_id).await {
                    tracing::error!("Failed to delete saga for {}: {}", saga.operation_id, e);
                    tx.rollback().await?;
                    continue;
                }

                tx.commit().await?;

                tracing::info!(
                    "Successfully recovered and compensated melt saga {}",
                    saga.operation_id
                );
            }
        }

        tracing::info!(
            "Successfully recovered {} incomplete melt sagas.",
            total_sagas
        );

        Ok(())
    }
}
