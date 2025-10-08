use std::collections::HashMap;

use cdk_common::amount::to_unit;
use cdk_common::database::{self, MintTransaction};
use cdk_common::mint::MeltQuote;
use cdk_common::nuts::{CurrencyUnit, MeltRequest};
use cdk_common::payment::DynMintPayment;
use cdk_common::quote_id::QuoteId;
use tracing::instrument;

use super::payment_executor::PaymentExecutor;
use crate::mint::Mint;
use crate::{Amount, Error};

/// Handles external melt operations where payment is made via payment processor
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
    /// Returns (transaction, preimage, amount_spent, quote)
    #[instrument(skip_all)]
    pub async fn execute<'b>(
        &self,
        tx: Box<dyn MintTransaction<'b, database::Error> + Send + Sync + 'b>,
        quote: &MeltQuote,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<
        (
            Box<dyn MintTransaction<'a, database::Error> + Send + Sync + 'a>,
            Option<String>,
            Amount,
            MeltQuote,
        ),
        Error,
    >
    where
        'b: 'a,
    {
        let _partial_amount = match quote.unit {
            CurrencyUnit::Sat | CurrencyUnit::Msat => {
                self.mint
                    .check_melt_expected_ln_fees(quote, melt_request)
                    .await?
            }
            _ => None,
        };

        tx.commit().await?;

        let payment_executor = PaymentExecutor::new(self.payment_processors.clone());
        let payment_result = payment_executor.execute_payment(quote).await?;

        let amount_spent =
            to_unit(payment_result.total_spent, &quote.unit, &quote.unit).unwrap_or_default();

        let mut tx = self.mint.localstore.begin_transaction().await?;

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

            if let Err(err) = tx
                .update_melt_quote_request_lookup_id(&quote.id, &payment_result.payment_lookup_id)
                .await
            {
                tracing::warn!("Could not update payment lookup id: {}", err);
            }
        }

        Ok((
            tx,
            payment_result.payment_proof,
            amount_spent,
            updated_quote,
        ))
    }
}
