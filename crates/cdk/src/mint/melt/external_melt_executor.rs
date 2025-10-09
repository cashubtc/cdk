use std::collections::HashMap;

use cdk_common::mint::MeltQuote;
use cdk_common::payment::DynMintPayment;
use tracing::instrument;

use super::payment_executor::PaymentExecutor;
use crate::mint::Mint;
use crate::{Amount, Error};

/// Handles external melt operations where payment is made via payment processor.
pub struct ExternalMeltExecutor<'a> {
    mint: &'a Mint,
    payment_processors: HashMap<crate::types::PaymentProcessorKey, DynMintPayment>,
}

impl<'a> ExternalMeltExecutor<'a> {
    pub fn new(
        mint: &'a Mint,
        payment_processors: HashMap<crate::types::PaymentProcessorKey, DynMintPayment>,
    ) -> Self {
        Self {
            mint,
            payment_processors,
        }
    }

    /// Execute external melt - make payment via payment processor
    ///
    /// Makes a payment using the configured payment processor for the quote's unit and method.
    /// The payment processor handles fee validation internally.
    ///
    /// # Returns
    /// A tuple of (payment preimage, amount spent, updated quote)
    #[instrument(skip_all)]
    pub async fn execute(
        &self,
        quote: &MeltQuote,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        tracing::info!(
            "Starting external melt execution for quote {} ({} {}, method: {})",
            quote.id,
            quote.amount,
            quote.unit,
            quote.payment_method
        );

        let payment_executor = PaymentExecutor::new(self.payment_processors.clone());

        let payment_result = payment_executor.execute_payment(quote).await?;

        tracing::info!(
            "Payment executed successfully for quote {} - total_spent: {}, payment_lookup_id: {}",
            quote.id,
            payment_result.total_spent,
            payment_result.payment_lookup_id
        );

        let mut updated_quote = quote.clone();
        if Some(payment_result.payment_lookup_id.clone()).as_ref()
            != quote.request_lookup_id.as_ref()
        {
            tracing::info!(
                "Payment lookup id changed post payment from {:?} to {}",
                &quote.request_lookup_id,
                payment_result.payment_lookup_id
            );

            updated_quote.request_lookup_id = Some(payment_result.payment_lookup_id.clone());

            tracing::debug!(
                "Updating payment lookup id in database for quote {}",
                quote.id
            );

            // Update the payment lookup ID in the database
            let mut tx = self.mint.localstore.begin_transaction().await?;
            if let Err(err) = tx
                .update_melt_quote_request_lookup_id(&quote.id, &payment_result.payment_lookup_id)
                .await
            {
                tracing::warn!(
                    "Could not update payment lookup id for quote {}: {}",
                    quote.id,
                    err
                );
            }
            tx.commit().await?;
        }

        Ok((
            payment_result.payment_proof,
            payment_result.total_spent,
            updated_quote,
        ))
    }
}
