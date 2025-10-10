//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use cdk_common::database;
use cdk_common::mint::Operation;

use super::{Error, Mint};
use crate::mint::{MeltQuote, MeltQuoteState, PaymentMethod};
use crate::nuts::State;
use crate::types::PaymentProcessorKey;

impl Mint {
    /// Checks the states of melt quotes that are **PENDING** or **UNKNOWN** to the mint with the ln node
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        // TODO: We should have a db query to do this filtering
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        let pending_quotes: Vec<MeltQuote> = melt_quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending || q.state == MeltQuoteState::Unknown)
            .collect();
        tracing::info!("There are {} pending melt quotes.", pending_quotes.len());

        if pending_quotes.is_empty() {
            return Ok(());
        }

        let mut tx = self.localstore.begin_transaction().await?;

        for pending_quote in pending_quotes {
            tracing::debug!("Checking status for melt quote {}.", pending_quote.id);

            let ln_key = PaymentProcessorKey {
                unit: pending_quote.unit,
                method: PaymentMethod::Bolt11,
            };

            let ln_backend = match self.payment_processors.get(&ln_key) {
                Some(ln_backend) => ln_backend,
                None => {
                    tracing::warn!("No backend for ln key: {:?}", ln_key);
                    continue;
                }
            };

            if let Some(lookup_id) = pending_quote.request_lookup_id {
                let pay_invoice_response = ln_backend.check_outgoing_payment(&lookup_id).await?;

                tracing::warn!(
                    "There is no stored melt request for pending melt quote: {}",
                    pending_quote.id
                );

                let melt_quote_state = match pay_invoice_response.status {
                    MeltQuoteState::Unpaid => MeltQuoteState::Unpaid,
                    MeltQuoteState::Paid => MeltQuoteState::Paid,
                    MeltQuoteState::Pending => MeltQuoteState::Pending,
                    MeltQuoteState::Failed => MeltQuoteState::Unpaid,
                    MeltQuoteState::Unknown => MeltQuoteState::Unpaid,
                };

                if let Err(err) = tx
                    .update_melt_quote_state(
                        &pending_quote.id,
                        melt_quote_state,
                        pay_invoice_response.payment_proof,
                    )
                    .await
                {
                    tracing::error!(
                        "Could not update quote {} to state {}, current state {}, {}",
                        pending_quote.id,
                        melt_quote_state,
                        pending_quote.state,
                        err
                    );
                };
            }
        }

        tx.commit().await?;

        Ok(())
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
