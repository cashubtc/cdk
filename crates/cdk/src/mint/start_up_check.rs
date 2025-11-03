//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use std::str::FromStr;

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

            // Use the same compensation logic as in-process failures
            let compensation = RemoveSwapSetup {
                blinded_secrets: saga.blinded_secrets.clone(),
                input_ys: saga.input_ys.clone(),
            };

            // Execute compensation
            if let Err(e) = compensation.execute(&self.localstore).await {
                tracing::error!(
                    "Failed to compensate saga {}: {}. Continuing...",
                    saga.operation_id,
                    e
                );
                continue;
            }

            // Delete saga after successful compensation
            let mut tx = self.localstore.begin_transaction().await?;
            if let Err(e) = tx.delete_saga(&saga.operation_id).await {
                tracing::error!("Failed to delete saga for {}: {}", saga.operation_id, e);
                tx.rollback().await?;
                continue;
            }
            tx.commit().await?;

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
                        let tx = self.localstore.begin_transaction().await?;
                        let proof_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;
                        tx.rollback().await?;

                        if !saga.input_ys.is_empty()
                            && !proof_ys.is_empty()
                            && saga.input_ys.iter().any(|y| proof_ys.contains(y))
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

            // Check saga state to determine if payment was sent
            // SetupComplete means setup transaction committed but payment NOT yet sent
            let should_compensate = match &saga.state {
                cdk_common::mint::SagaStateEnum::Melt(state) => {
                    match state {
                        cdk_common::mint::MeltSagaState::SetupComplete => {
                            // Setup complete but payment never sent - always compensate
                            tracing::info!(
                                "Saga {} in SetupComplete state - payment never sent, will compensate",
                                saga.operation_id
                            );
                            true
                        }
                        _ => {
                            // Other states - should not happen in incomplete sagas, but check payment status anyway
                            false // Will check payment status below
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
                // Use saga data directly for compensation (like swap does)
                tracing::info!(
                    "Compensating melt saga {} (removing {} proofs, {} change outputs)",
                    saga.operation_id,
                    saga.input_ys.len(),
                    saga.blinded_secrets.len()
                );

                // Compensate using saga data only - don't rely on quote state
                let mut tx = self.localstore.begin_transaction().await?;

                // Remove blinded messages (change outputs)
                if !saga.blinded_secrets.is_empty() {
                    if let Err(e) = tx.delete_blinded_messages(&saga.blinded_secrets).await {
                        tracing::error!(
                            "Failed to delete blinded messages for saga {}: {}",
                            saga.operation_id,
                            e
                        );
                        tx.rollback().await?;
                        continue;
                    }
                }

                // Remove proofs (inputs) - use None for quote_id like swap does
                if !saga.input_ys.is_empty() {
                    if let Err(e) = tx.remove_proofs(&saga.input_ys, None).await {
                        tracing::error!(
                            "Failed to remove proofs for saga {}: {}",
                            saga.operation_id,
                            e
                        );
                        tx.rollback().await?;
                        continue;
                    }
                }

                // Reset quote state to Unpaid (melt-specific, unlike swap)
                if let Err(e) = tx
                    .update_melt_quote_state(&quote_id_parsed, MeltQuoteState::Unpaid, None)
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
