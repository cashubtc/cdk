use std::collections::HashMap;

use cdk_common::amount::to_unit;
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
    /// Returns (preimage, amount_spent, quote)
    #[instrument(skip_all)]
    pub async fn execute(
        &self,
        quote: &MeltQuote,
    ) -> Result<(Option<String>, Amount, MeltQuote), Error> {
        let payment_executor = PaymentExecutor::new(self.payment_processors.clone());
        let payment_result = payment_executor.execute_payment(quote).await?;

        let amount_spent =
            to_unit(payment_result.total_spent, &quote.unit, &quote.unit).unwrap_or_default();

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

            // Update the payment lookup ID in the database
            let mut tx = self.mint.localstore.begin_transaction().await?;
            if let Err(err) = tx
                .update_melt_quote_request_lookup_id(&quote.id, &payment_result.payment_lookup_id)
                .await
            {
                tracing::warn!("Could not update payment lookup id: {}", err);
            }
            tx.commit().await?;
        }

        Ok((payment_result.payment_proof, amount_spent, updated_quote))
    }
}
