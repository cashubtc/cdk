//! Check used at mint start up
//!
//! These checks are needed in the case the mint was offline and the payment backend was not.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the backend.

use std::str::FromStr;

use cdk_common::database::DynMintTransaction;
use cdk_common::mint::{MeltPaymentRequest, OperationKind, Saga};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{PublicKey, QuoteId, State};

use super::{Error, Mint};
use crate::mint::swap::swap_saga::compensation::{CompensatingAction, RemoveSwapSetup};
use crate::mint::{MeltQuote, MeltQuoteState};
use crate::types::PaymentProcessorKey;

/// Recovery decision for an incomplete swap saga found during startup.
#[derive(Debug, PartialEq, Eq)]
enum SwapSagaRecoveryAction {
    /// Roll back the pre-finalization setup state with the normal swap compensation.
    Compensate,
    /// Leave the saga alone because it was cleaned up or requires manual repair.
    SkipCompensation,
}

impl Mint {
    /// Get incomplete melt saga by quote_id
    async fn get_melt_saga_by_quote_id(&self, quote_id: &str) -> Result<Option<Saga>, Error> {
        let incomplete_sagas = self
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await?;

        for saga in incomplete_sagas {
            if let Some(ref qid) = saga.quote_id {
                if qid == quote_id {
                    return Ok(Some(saga));
                }
            }
        }
        Ok(None)
    }

    /// Returns the stable identifier a payment backend receives for this
    /// quote, including quotes created before that identifier was persisted.
    fn melt_payment_lookup_id(quote: &MeltQuote) -> PaymentIdentifier {
        quote
            .request_lookup_id
            .clone()
            .unwrap_or_else(|| match &quote.request {
                MeltPaymentRequest::Bolt11 { bolt11 } => {
                    PaymentIdentifier::PaymentHash(*bolt11.payment_hash().as_ref())
                }
                MeltPaymentRequest::Bolt12 { .. }
                | MeltPaymentRequest::Custom { .. }
                | MeltPaymentRequest::Onchain { .. } => {
                    PaymentIdentifier::QuoteId(quote.id.clone())
                }
            })
    }

    /// Checks the payment status of a melt quote with the payment backend
    ///
    /// This is a helper function used by saga recovery to determine whether to
    /// finalize or compensate an incomplete melt operation.
    ///
    /// # Returns
    ///
    /// - `Ok(MakePaymentResponse)`: Payment status successfully retrieved from backend
    /// - `Err(Error)`: Failed to check payment status (for example, backend unavailable)
    pub(super) async fn check_melt_payment_status(
        &self,
        quote: &MeltQuote,
    ) -> Result<crate::cdk_payment::MakePaymentResponse, Error> {
        let payment_processor_key = PaymentProcessorKey {
            unit: quote.unit.clone(),
            method: quote.payment_method.clone(),
        };

        let payment_backend = self
            .payment_processors
            .get(&payment_processor_key)
            .ok_or_else(|| {
                tracing::warn!(
                    "No backend for payment processor key: {:?}",
                    payment_processor_key
                );
                Error::UnsupportedUnit
            })?;

        let lookup_id = Self::melt_payment_lookup_id(quote);

        // Check payment status with the payment backend
        let pay_invoice_response = payment_backend
            .check_outgoing_payment(&lookup_id)
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
    /// Uses shared finalization logic from melt::shared module.
    /// The `operation_id` is passed through to record the completed operation
    /// and delete the saga atomically.
    async fn finalize_paid_melt_quote(
        &self,
        quote: &MeltQuote,
        total_spent: cdk_common::Amount<cdk_common::CurrencyUnit>,
        payment_proof: Option<String>,
        payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
        operation_id: uuid::Uuid,
    ) -> Result<(), Error> {
        tracing::info!("Finalizing paid melt quote {} during startup", quote.id);

        // Use shared finalization — handles operation recording and saga
        // deletion atomically
        super::melt::shared::finalize_melt_quote(
            self,
            &self.localstore,
            &self.pubsub_manager,
            quote,
            total_spent,
            payment_proof,
            payment_lookup_id,
            Some(operation_id),
        )
        .await?;

        tracing::info!(
            "Successfully finalized melt quote {} during startup check",
            quote.id
        );

        Ok(())
    }

    /// Returns the synthetic paid response for a melt settled on this mint.
    ///
    /// Internal settlements do not reach a payment backend, so recovery must
    /// reconstruct the response needed by the shared finalization path. A
    /// non-internal melt returns `Ok(None)`; database errors are propagated so
    /// callers can fail closed according to their recovery context.
    pub(crate) async fn internal_melt_settlement_response(
        &self,
        quote: &MeltQuote,
    ) -> Result<Option<crate::cdk_payment::MakePaymentResponse>, Error> {
        let mut tx = self.localstore.begin_transaction().await?;
        let response = Self::internal_melt_settlement_response_tx(&mut tx, quote).await;
        tx.rollback().await?;
        response
    }

    /// Transaction-scoped internal-settlement check used by reconciliation.
    pub(crate) async fn internal_melt_settlement_response_tx(
        tx: &mut DynMintTransaction,
        quote: &MeltQuote,
    ) -> Result<Option<crate::cdk_payment::MakePaymentResponse>, Error> {
        let Some(mint_quote) = tx
            .get_mint_quote_by_request(&quote.request.to_string())
            .await?
        else {
            return Ok(None);
        };

        let melt_quote_id = quote.id.to_string();
        if !mint_quote.payment_ids().contains(&&melt_quote_id) {
            return Ok(None);
        }

        let payment_lookup_id = quote.request_lookup_id.clone().unwrap_or_else(|| {
            cdk_common::payment::PaymentIdentifier::CustomId(quote.id.to_string())
        });

        Ok(Some(crate::cdk_payment::MakePaymentResponse {
            payment_lookup_id,
            payment_proof: None,
            status: MeltQuoteState::Paid,
            total_spent: quote.amount(),
        }))
    }

    async fn recover_legacy_finalizing_saga(
        &self,
        saga: &Saga,
        quote: &MeltQuote,
    ) -> Result<Option<crate::cdk_payment::MakePaymentResponse>, Error> {
        if let Some(payment_response) = self.internal_melt_settlement_response(quote).await? {
            tracing::info!(
                "Legacy Finalizing saga {} identified as internal settlement",
                saga.operation_id
            );

            return Ok(Some(payment_response));
        }

        match self.check_melt_payment_status(quote).await {
            Ok(payment_response) if payment_response.status == MeltQuoteState::Paid => {
                tracing::info!(
                    "Recovered legacy Finalizing saga {} from the payment backend",
                    saga.operation_id
                );
                Ok(Some(payment_response))
            }
            Ok(payment_response) => {
                tracing::error!(
                    "Legacy Finalizing saga {} returned {} from the payment backend after TX1 may have committed. Manual intervention required.",
                    saga.operation_id,
                    payment_response.status
                );
                Ok(None)
            }
            Err(err) => {
                tracing::error!(
                    "Failed to recover legacy Finalizing saga {} from the payment backend: {}",
                    saga.operation_id,
                    err
                );
                Ok(None)
            }
        }
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

            match self
                .cleanup_finalized_swap_saga(&saga, &input_ys, &blinded_secrets)
                .await?
            {
                SwapSagaRecoveryAction::SkipCompensation => continue,
                SwapSagaRecoveryAction::Compensate => {}
            }

            // Use the same compensation logic as in-process failures
            // Saga deletion is included in the compensation transaction
            let compensation = RemoveSwapSetup {
                blinded_secrets,
                input_ys,
                operation_id: saga.operation_id,
            };

            // Execute compensation (includes saga deletion)
            if let Err(e) = compensation
                .execute(&self.localstore, &self.pubsub_manager)
                .await
            {
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

    async fn cleanup_finalized_swap_saga(
        &self,
        saga: &Saga,
        input_ys: &[PublicKey],
        blinded_secrets: &[PublicKey],
    ) -> Result<SwapSagaRecoveryAction, Error> {
        if input_ys.is_empty() || blinded_secrets.is_empty() {
            return Ok(SwapSagaRecoveryAction::Compensate);
        }

        let proof_states = self.localstore.get_proofs_states(input_ys).await?;
        let inputs_spent = proof_states
            .iter()
            .all(|state| matches!(state, Some(State::Spent)));

        if !inputs_spent {
            return Ok(SwapSagaRecoveryAction::Compensate);
        }

        let output_signatures = self
            .localstore
            .get_blind_signatures(blinded_secrets)
            .await?;
        let outputs_signed = output_signatures.iter().all(Option::is_some);

        if !outputs_signed {
            tracing::error!(
                "Swap saga {} has spent inputs but missing output signatures; manual intervention required",
                saga.operation_id
            );
            return Ok(SwapSagaRecoveryAction::SkipCompensation);
        }

        tracing::info!(
            "Swap saga {} already finalized; deleting orphaned saga record",
            saga.operation_id
        );

        match self.localstore.begin_transaction().await {
            Ok(mut tx) => {
                if let Err(err) = tx.delete_saga(&saga.operation_id).await {
                    tracing::warn!(
                        "Failed to delete finalized orphaned swap saga {}: {}",
                        saga.operation_id,
                        err
                    );
                    if let Err(rollback_err) = tx.rollback().await {
                        tracing::warn!(
                            "Failed to roll back finalized orphaned swap saga cleanup {}: {}",
                            saga.operation_id,
                            rollback_err
                        );
                    }
                } else if let Err(err) = tx.commit().await {
                    tracing::warn!(
                        "Failed to commit finalized orphaned swap saga cleanup {}: {}",
                        saga.operation_id,
                        err
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to start finalized orphaned swap saga cleanup {}: {}",
                    saga.operation_id,
                    err
                );
            }
        }

        Ok(SwapSagaRecoveryAction::SkipCompensation)
    }

    /// Recover from incomplete melt sagas
    ///
    /// Checks all persisted sagas for melt operations and determines whether to:
    /// - **Finalize**: If payment was confirmed as PAID on the payment backend, the
    ///   saga already reached `Finalizing`, or the melt settled internally
    /// - **Compensate**: If the saga is `SetupComplete` (payment never
    ///   attempted), or `PaymentFailed` (authoritative failure was recorded),
    ///   or the backend reports an authoritative `Unpaid`/`Failed` status
    /// - **Skip**: For `Pending`/`Unknown` polling results, which are not
    ///   authoritative because an orchestrator may be between attempts
    ///
    /// This recovery handles SetupComplete state which means:
    /// - Proofs were reserved (marked as PENDING)
    /// - Change outputs were added
    /// - Payment was never attempted (the write-ahead marker advances the saga
    ///   to PaymentAttempted before any dispatch)
    ///
    /// # Critical Bug Fix
    ///
    /// Previously, this function always compensated (rolled back) incomplete sagas without
    /// checking if the payment actually succeeded on the payment backend. This could cause the
    /// mint to lose funds if:
    /// 1. Payment succeeded on the payment backend
    /// 2. Mint crashed before finalize() committed
    /// 3. Recovery compensated (returned proofs) instead of finalizing
    ///
    /// Recovery checks backend status to finalize a paid result, and to
    /// compensate an authoritative `Unpaid`/`Failed` result. `Pending` and
    /// `Unknown` cannot compensate a payment that may still be retried.
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

            // Startup has no live dispatches, but this recovery method is also
            // callable directly. Serialize it with any in-process melt or
            // quote check, then refresh the saga state acquired before waiting.
            let quote_lock = self.melt_quote_lock(&quote_id_parsed).await;
            let _quote_guard = quote_lock.lock_owned().await;
            let Some(saga) = self
                .localstore
                .get_melt_saga_by_quote_id(&quote_id_parsed)
                .await?
            else {
                continue;
            };

            let input_ys = self
                .localstore
                .get_proof_ys_by_operation_id(&saga.operation_id)
                .await?;
            let blinded_secrets = self
                .localstore
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;

            let mut quote = match self.localstore.get_melt_quote(&quote_id_parsed).await {
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
            // PaymentAttempted means payment was attempted - must check the payment backend
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
                        cdk_common::mint::MeltSagaState::PaymentFailed => {
                            tracing::info!(
                                "Saga {} has an authoritative payment failure - will compensate",
                                saga.operation_id
                            );
                            true
                        }
                        cdk_common::mint::MeltSagaState::Finalizing => {
                            // TX1 committed (proofs Spent, quote Paid) - finalize change + cleanup
                            tracing::info!(
                                "Saga {} in Finalizing state - TX1 completed, will finalize change and cleanup",
                                saga.operation_id
                            );

                            let payment_response = match saga.finalization_data.clone() {
                                Some(finalization_data) => {
                                    crate::cdk_payment::MakePaymentResponse {
                                        payment_lookup_id: finalization_data.payment_lookup_id,
                                        payment_proof: finalization_data.payment_proof,
                                        status: MeltQuoteState::Paid,
                                        total_spent: finalization_data.total_spent,
                                    }
                                }
                                None => {
                                    let Some(payment_response) = (match self
                                        .recover_legacy_finalizing_saga(&saga, &quote)
                                        .await
                                    {
                                        Ok(payment_response) => payment_response,
                                        Err(err) => {
                                            tracing::error!(
                                                "Failed to recover legacy Finalizing saga {}: {}. Skipping.",
                                                saga.operation_id,
                                                err
                                            );
                                            continue;
                                        }
                                    }) else {
                                        continue;
                                    };
                                    payment_response
                                }
                            };
                            let payment_lookup_id = payment_response.payment_lookup_id.clone();
                            let payment_proof = payment_response.payment_proof.clone();

                            if let Err(err) = self
                                .finalize_paid_melt_quote(
                                    &quote,
                                    payment_response.total_spent,
                                    payment_proof.clone(),
                                    &payment_lookup_id,
                                    saga.operation_id,
                                )
                                .await
                            {
                                tracing::error!(
                                    "Failed to finalize Finalizing saga {}: {}. Will retry.",
                                    saga.operation_id,
                                    err
                                );
                                continue;
                            }

                            quote.state = MeltQuoteState::Paid;
                            quote.payment_proof = payment_proof;
                            quote.request_lookup_id = Some(payment_lookup_id);

                            tracing::info!(
                                "Successfully recovered Finalizing saga {}",
                                saga.operation_id
                            );
                            continue;
                        }
                        cdk_common::mint::MeltSagaState::PaymentAttempted
                        | cdk_common::mint::MeltSagaState::PaymentPending => {
                            // Payment was attempted - check for internal settlement first, then the payment backend
                            tracing::info!(
                                "Saga {} in {} state - checking for internal or external payment",
                                saga.operation_id,
                                state
                            );

                            // Check if this was an internal settlement by looking for a mint quote
                            // that was paid by this melt quote
                            let internal_payment_response = match self
                                .internal_melt_settlement_response(&quote)
                                .await
                            {
                                Ok(payment_response) => payment_response,
                                Err(err) => {
                                    // Fail closed: if internal settlement cannot be
                                    // determined, never compensate; retry on the next
                                    // recovery cycle.
                                    tracing::error!(
                                        "Failed to determine internal settlement for saga {}: {}. Leaving pending.",
                                        saga.operation_id,
                                        err
                                    );
                                    continue;
                                }
                            };

                            if let Some(payment_response) = internal_payment_response {
                                // Internal settlement was completed - finalize directly
                                tracing::info!(
                                    "Saga {} was internal settlement - will finalize directly",
                                    saga.operation_id
                                );

                                let payment_lookup_id = payment_response.payment_lookup_id;

                                if let Err(err) = self
                                    .finalize_paid_melt_quote(
                                        &quote,
                                        payment_response.total_spent,
                                        payment_response.payment_proof,
                                        &payment_lookup_id,
                                        saga.operation_id,
                                    )
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to finalize internal settlement saga {}: {}. Will retry on next recovery cycle.",
                                        saga.operation_id,
                                        err
                                    );
                                    continue;
                                }

                                tracing::info!(
                                    "Successfully recovered and finalized internal settlement saga {}",
                                    saga.operation_id
                                );

                                continue; // Skip to next saga
                            }

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
            } else {
                // Payment was attempted - check payment backend status
                tracing::info!(
                    "Saga {} for quote {} was attempted - checking payment status with backend",
                    saga.operation_id,
                    quote_id
                );

                match self.check_melt_payment_status(&quote).await {
                    Ok(payment_response) => {
                        match payment_response.status {
                            MeltQuoteState::Paid => {
                                if let Err(err) = super::saga_recovery::process_melt_saga_outcome(
                                    &saga,
                                    &mut quote,
                                    &payment_response,
                                    &self.localstore,
                                    &self.pubsub_manager,
                                    self,
                                )
                                .await
                                {
                                    tracing::error!(
                                        "Failed to process paid melt saga {}: {}. Will retry on next recovery cycle.",
                                        saga.operation_id,
                                        err
                                    );
                                    continue;
                                }
                                continue; // Saga handled
                            }
                            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                                // A negative status is an authoritative
                                // terminal result by backend contract (see
                                // MintPayment::check_outgoing_payment).
                                if let Err(err) = super::saga_recovery::process_melt_saga_outcome(
                                    &saga,
                                    &mut quote,
                                    &payment_response,
                                    &self.localstore,
                                    &self.pubsub_manager,
                                    self,
                                )
                                .await
                                {
                                    tracing::error!(
                                        "Failed to process failed melt saga {}: {}. Will retry on next recovery cycle.",
                                        saga.operation_id,
                                        err
                                    );
                                    continue;
                                }
                                continue; // Saga handled
                            }
                            MeltQuoteState::Pending | MeltQuoteState::Unknown => {
                                // Not authoritative: an orchestrator may be
                                // between payment attempts.
                                tracing::info!(
                                    "Saga {} for quote {} - payment {} on the payment backend, skipping",
                                    saga.operation_id,
                                    quote_id,
                                    payment_response.status
                                );
                                continue; // Skip this saga
                            }
                        }
                    }
                    Err(err) => {
                        // payment backend unavailable - skip this saga, will retry on next recovery cycle
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

                let rollback = match &saga.state {
                    cdk_common::mint::SagaStateEnum::Melt(
                        cdk_common::mint::MeltSagaState::SetupComplete,
                    ) => {
                        super::melt::shared::rollback_setup_melt_quote(
                            &self.localstore,
                            &self.pubsub_manager,
                            &quote_id_parsed,
                            &input_ys,
                            &blinded_secrets,
                            &saga.operation_id,
                        )
                        .await
                    }
                    cdk_common::mint::SagaStateEnum::Melt(
                        cdk_common::mint::MeltSagaState::PaymentFailed,
                    ) => {
                        super::melt::shared::rollback_failed_melt_quote(
                            &self.localstore,
                            &self.pubsub_manager,
                            &quote_id_parsed,
                            &input_ys,
                            &blinded_secrets,
                            &saga.operation_id,
                        )
                        .await
                    }
                    _ => continue,
                };

                if let Err(err) = rollback {
                    tracing::error!(
                        "Failed to rollback melt quote {} for saga {}: {}",
                        quote_id_parsed,
                        saga.operation_id,
                        err
                    );
                }
            }
        }

        tracing::info!(
            "Successfully recovered {} incomplete melt sagas.",
            total_sagas
        );

        Ok(())
    }

    /// Handle pending melt quote by resuming the saga
    pub(crate) async fn handle_pending_melt_quote(
        &self,
        quote: &mut MeltQuote,
    ) -> Result<(), Error> {
        let quote_lock = self.melt_quote_lock(&quote.id).await;
        let _quote_guard = quote_lock.lock_owned().await;

        // The caller may have loaded the quote before waiting for an active
        // dispatch/finalization. Refresh it after acquiring the guard.
        *quote = self
            .localstore
            .get_melt_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let saga = match self
            .get_melt_saga_by_quote_id(&quote.id.to_string())
            .await?
        {
            Some(saga) => saga,
            None => {
                if quote.state == MeltQuoteState::Pending {
                    tracing::warn!(
                        "No saga found for pending melt quote {}, cannot resume",
                        quote.id
                    );
                }
                return Ok(());
            }
        };

        if saga.state
            == cdk_common::mint::SagaStateEnum::Melt(cdk_common::mint::MeltSagaState::PaymentFailed)
        {
            return super::saga_recovery::recover_recorded_payment_failure(
                &saga,
                quote,
                &self.localstore,
                &self.pubsub_manager,
            )
            .await;
        }

        // An internal settlement commits the mint-quote credit and marks this
        // quote Paid in one transaction, so the quote may be Paid here while
        // finalization (spending proofs, change, saga cleanup) is still
        // outstanding. Resume it on demand rather than waiting for startup
        // recovery.
        if let Some(payment_response) = self.internal_melt_settlement_response(quote).await? {
            return super::saga_recovery::process_melt_saga_outcome(
                &saga,
                quote,
                &payment_response,
                &self.localstore,
                &self.pubsub_manager,
                self,
            )
            .await;
        }

        if quote.state != MeltQuoteState::Pending {
            return Ok(());
        }

        let payment_response = self.check_melt_payment_status(quote).await?;

        super::saga_recovery::process_melt_saga_outcome(
            &saga,
            quote,
            &payment_response,
            &self.localstore,
            &self.pubsub_manager,
            self,
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nuts::ProofsMethods;
    use cdk_common::{Amount, State};

    use super::*;
    use crate::mint::swap::swap_saga::SwapSaga;
    use crate::test_helpers::mint::{
        create_test_blinded_messages, create_test_mint, mint_test_proofs,
    };

    #[tokio::test]
    async fn spent_swap_inputs_with_unsigned_outputs_skip_compensation() {
        let mint = create_test_mint().await.unwrap();
        let db = mint.localstore();

        let amount = Amount::from(100);
        let input_proofs = mint_test_proofs(&mint, amount).await.unwrap();
        let input_ys = input_proofs.ys().unwrap();
        let input_verification = crate::mint::Verification {
            amount: amount.with_unit(cdk_common::nuts::CurrencyUnit::Sat),
        };
        let (output_blinded_messages, _) =
            create_test_blinded_messages(&mint, amount).await.unwrap();
        let blinded_secrets: Vec<_> = output_blinded_messages
            .iter()
            .map(|message| message.blinded_secret)
            .collect();

        let saga = SwapSaga::new(&mint, db.clone(), mint.pubsub_manager())
            .setup_swap(
                &input_proofs,
                &output_blinded_messages,
                None,
                input_verification,
            )
            .await
            .expect("setup should succeed");
        drop(saga);

        let operation_id = db
            .get_incomplete_sagas(OperationKind::Swap)
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("saga should exist")
            .operation_id;

        {
            let mut tx = db.begin_transaction().await.unwrap();
            let mut proofs = tx.get_proofs(&input_ys).await.unwrap();
            Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        let saga = {
            let mut tx = db.begin_transaction().await.unwrap();
            let saga = tx
                .get_saga(&operation_id)
                .await
                .unwrap()
                .expect("saga should exist");
            tx.commit().await.unwrap();
            saga
        };

        let action = mint
            .cleanup_finalized_swap_saga(&saga, &input_ys, &blinded_secrets)
            .await
            .unwrap();

        assert_eq!(action, SwapSagaRecoveryAction::SkipCompensation);

        let saga_after = {
            let mut tx = db.begin_transaction().await.unwrap();
            let saga = tx.get_saga(&operation_id).await.unwrap();
            tx.commit().await.unwrap();
            saga
        };
        assert!(
            saga_after.is_some(),
            "manual-intervention saga should remain for repair"
        );
    }
}
