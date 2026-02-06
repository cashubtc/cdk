//! Shared saga recovery logic for melt operations.
//!
//! This module contains functions used by both startup recovery and on-demand quote checking
//! to process melt saga outcomes consistently.

use cdk_common::database::DynMintDatabase;
use cdk_common::mint::{MeltQuote, Saga};
use cdk_common::nuts::MeltQuoteState;
use cdk_common::payment::MakePaymentResponse;
use tracing::instrument;

use crate::mint::subscription::PubSubManager;
use crate::mint::Mint;
use crate::Error;

/// Process the outcome of a melt saga based on LN payment status.
///
/// This function handles the shared logic for deciding whether to finalize, compensate, or skip
/// a melt operation based on the payment response from the LN backend.
///
/// # Arguments
/// * `saga` - The melt saga being processed
/// * `quote` - The melt quote associated with the saga
/// * `payment_response` - The payment status from the LN backend
/// * `db` - Database handle
/// * `pubsub` - PubSub manager for notifications
/// * `mint` - Mint instance for signing operations
///
/// # Returns
/// Ok(()) on success, or an error if processing fails
#[instrument(skip_all)]
pub(crate) async fn process_melt_saga_outcome(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    match payment_response.status {
        MeltQuoteState::Paid => {
            tracing::info!(
                "Finalizing paid melt quote {} (saga {})",
                quote.id,
                saga.operation_id
            );
            super::melt::shared::finalize_melt_quote(
                mint,
                db,
                pubsub,
                quote,
                payment_response.total_spent.clone(),
                payment_response.payment_proof.clone(),
                &payment_response.payment_lookup_id,
            )
            .await?;
            // Delete saga after successful finalization
            let mut tx = db.begin_transaction().await?;
            tx.delete_saga(&saga.operation_id).await?;
            tx.commit().await?;
        }
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
            tracing::info!(
                "Compensating failed melt quote {} (saga {})",
                quote.id,
                saga.operation_id
            );
            let input_ys = db.get_proof_ys_by_operation_id(&saga.operation_id).await?;
            let blinded_secrets = db
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;
            super::melt::shared::rollback_melt_quote(
                db,
                pubsub,
                &quote.id,
                &input_ys,
                &blinded_secrets,
                &saga.operation_id,
            )
            .await?;

            quote.state = MeltQuoteState::Unpaid;
        }
        MeltQuoteState::Pending | MeltQuoteState::Unknown => {
            tracing::debug!(
                "Melt quote {} (saga {}) payment status still {}, skipping action",
                quote.id,
                saga.operation_id,
                payment_response.status
            );
        }
    }
    Ok(())
}
