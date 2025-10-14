//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use cdk_common::database;
use cdk_common::mint::Operation;

use super::{Error, Mint};
use crate::mint::{MeltQuote, MeltQuoteState};
use crate::nuts::State;
use crate::types::PaymentProcessorKey;

impl Mint {
    /// Checks the states of melt quotes that are **PENDING** or **UNKNOWN** to the mint with the ln node
    ///
    /// This function handles recovery from:
    /// - Mint restart while payment was in progress
    /// - Network interruption during payment
    /// - Database transaction failure during finalization
    ///
    /// For each pending melt quote:
    /// - **PAID**: Completes the melt by marking proofs SPENT, signing change, and cleaning up
    /// - **UNPAID/FAILED**: Rolls back the melt by removing proofs, change outputs, and resetting quote
    /// - **PENDING/UNKNOWN**: Leaves proofs PENDING for manual intervention or future check
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        let pending_quotes: Vec<MeltQuote> = melt_quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending || q.state == MeltQuoteState::Unknown)
            .collect();

        tracing::info!("There are {} pending melt quotes.", pending_quotes.len());

        if pending_quotes.is_empty() {
            return Ok(());
        }

        for pending_quote in pending_quotes {
            tracing::debug!("Checking status for melt quote {}.", pending_quote.id);

            let ln_key = PaymentProcessorKey {
                unit: pending_quote.unit.clone(),
                method: pending_quote.payment_method.clone(),
            };

            let ln_backend = match self.payment_processors.get(&ln_key) {
                Some(ln_backend) => ln_backend,
                None => {
                    tracing::warn!("No backend for ln key: {:?}", ln_key);
                    continue;
                }
            };

            let lookup_id = match &pending_quote.request_lookup_id {
                Some(id) => id,
                None => {
                    tracing::warn!(
                        "No lookup_id for pending melt quote {}, cannot check status",
                        pending_quote.id
                    );
                    continue;
                }
            };

            // Check payment status with LN backend
            let pay_invoice_response = match ln_backend.check_outgoing_payment(lookup_id).await {
                Ok(response) => response,
                Err(err) => {
                    tracing::error!(
                        "Failed to check payment status for quote {}: {}",
                        pending_quote.id,
                        err
                    );
                    continue;
                }
            };

            tracing::info!(
                "Payment status for melt quote {}: {}",
                pending_quote.id,
                pay_invoice_response.status
            );

            // Handle based on payment status
            match pay_invoice_response.status {
                MeltQuoteState::Paid => {
                    // Payment succeeded - complete the melt (finalize)
                    if let Err(err) = self
                        .finalize_paid_melt_quote(
                            &pending_quote,
                            pay_invoice_response.total_spent,
                            pay_invoice_response.payment_proof.clone(),
                            &pay_invoice_response.payment_lookup_id,
                        )
                        .await
                    {
                        tracing::error!(
                            "Failed to finalize paid melt quote {}: {}",
                            pending_quote.id,
                            err
                        );
                    }
                }
                MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                    // Payment failed - rollback the melt (compensate)
                    if let Err(err) = self.rollback_failed_melt_quote(&pending_quote).await {
                        tracing::error!(
                            "Failed to rollback failed melt quote {}: {}",
                            pending_quote.id,
                            err
                        );
                    }
                }
                MeltQuoteState::Pending | MeltQuoteState::Unknown => {
                    // Payment still pending or unknown - leave as is for now
                    tracing::info!(
                        "Melt quote {} still {} - proofs remain pending",
                        pending_quote.id,
                        pay_invoice_response.status
                    );

                    // Update quote state to match backend state
                    let mut tx = self.localstore.begin_transaction().await?;
                    if let Err(err) = tx
                        .update_melt_quote_state(
                            &pending_quote.id,
                            pay_invoice_response.status,
                            None,
                        )
                        .await
                    {
                        tracing::error!(
                            "Failed to update quote {} state to {}: {}",
                            pending_quote.id,
                            pay_invoice_response.status,
                            err
                        );
                        tx.rollback().await?;
                    } else {
                        tx.commit().await?;
                    }
                }
            }
        }

        Ok(())
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

    /// Rolls back a failed melt quote during startup check
    ///
    /// Uses shared rollback logic from melt::shared module
    async fn rollback_failed_melt_quote(&self, quote: &MeltQuote) -> Result<(), Error> {
        tracing::info!("Rolling back failed melt quote {} during startup", quote.id);

        let mut tx = self.localstore.begin_transaction().await?;

        // Get melt request info to find change outputs
        let melt_request_info = match tx.get_melt_request_and_blinded_messages(&quote.id).await? {
            Some(info) => info,
            None => {
                tracing::warn!(
                    "No melt request found for quote {} - may have been rolled back already",
                    quote.id
                );
                tx.rollback().await?;
                return Ok(());
            }
        };

        // Get input proofs by quote_id
        let input_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;

        // Extract blinded secrets
        let blinded_secrets: Vec<_> = melt_request_info
            .change_outputs
            .iter()
            .map(|bm| bm.blinded_secret)
            .collect();

        // Rollback is done in a new transaction, so close this one
        tx.rollback().await?;

        // Use shared rollback function
        super::melt::shared::rollback_melt_quote(
            &self.localstore,
            &quote.id,
            &input_ys,
            &blinded_secrets,
        )
        .await
    }

    /// Recover from bad swap operations
    ///
    /// Checks all PENDING proofs where operation_kind is "swap".
    /// For each unique operation_id:
    /// - If blind signatures exist for that operation_id, mark the proofs as SPENT
    /// - If no blind signatures exist for that operation_id, remove the proofs from the database
    pub async fn recover_from_bad_swaps(&self) -> Result<(), Error> {
        use cdk_common::nut00::ProofsMethods;

        let mut tx = self.localstore.begin_transaction().await?;

        let pending_swap_proofs_by_operation = tx
            .get_proofs_by_state_and_operation_kind(State::Pending, "swap")
            .await?;

        if pending_swap_proofs_by_operation.is_empty() {
            tracing::info!("No pending swap proofs found to recover.");
            tx.commit().await?;
            return Ok(());
        }

        let total_proofs: usize = pending_swap_proofs_by_operation
            .values()
            .map(|v| v.len())
            .sum();
        let operation_count = pending_swap_proofs_by_operation.len();
        tracing::info!(
            "Found {} pending swap proofs in {} operations to recover from bad swaps.",
            total_proofs,
            operation_count
        );

        for (operation_id, proofs) in pending_swap_proofs_by_operation {
            tracing::debug!(
                "Checking operation_id {} with {} proofs",
                operation_id,
                proofs.len()
            );

            let operation = Operation::Swap(operation_id);

            // Check if blind signatures exist for this operation_id
            let blind_signatures = tx.get_blind_signatures_by_operation(&operation).await?;

            let proof_ys: Vec<_> = proofs.ys()?;

            if !blind_signatures.is_empty() {
                // Blind signatures exist, mark proofs as SPENT
                tracing::info!(
                    "Operation {} has {} blind signatures, marking {} proofs as SPENT",
                    operation_id,
                    blind_signatures.len(),
                    proof_ys.len()
                );

                match tx.update_proofs_states(&proof_ys, State::Spent).await {
                    Ok(_) => {}
                    Err(database::Error::AttemptUpdateSpentProof) => {
                        // Already processed - skip
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                // No blind signatures exist, remove the proofs
                tracing::info!(
                    "Operation {} has no blind signatures, removing {} proofs",
                    operation_id,
                    proof_ys.len()
                );

                if let Err(err) = tx.remove_proofs(&proof_ys, None).await {
                    tracing::error!(
                        "Failed to remove proofs for operation {}: {}",
                        operation_id,
                        err
                    );
                    tx.rollback().await?;
                    return Err(err.into());
                }

                if let Err(err) = tx.delete_blind_signatures_by_operation(&operation).await {
                    tracing::error!(
                        "Failed to remove proofs for operation {}: {}",
                        operation_id,
                        err
                    );
                    tx.rollback().await?;
                    return Err(err.into());
                }
            }
        }

        tx.commit().await?;

        tracing::info!(
            "Successfully recovered from bad swaps by processing {} operations.",
            operation_count
        );
        Ok(())
    }
}
