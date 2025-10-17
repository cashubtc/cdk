//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use cdk_common::mint::OperationKind;

use super::{Error, Mint};
use crate::mint::swap::swap_saga::compensation::{CompensatingAction, RemoveSwapSetup};
use crate::mint::{MeltQuote, MeltQuoteState, PaymentMethod};
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

    /// Recover from incomplete swap sagas
    ///
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
                saga.state.as_str(),
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

            // Delete saga state after successful compensation
            let mut tx = self.localstore.begin_transaction().await?;
            if let Err(e) = tx.delete_saga_state(&saga.operation_id).await {
                tracing::error!(
                    "Failed to delete saga state for {}: {}",
                    saga.operation_id,
                    e
                );
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
}
