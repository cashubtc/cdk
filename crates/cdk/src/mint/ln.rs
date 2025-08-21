use cdk_common::amount::to_unit;
use cdk_common::common::PaymentProcessorKey;
use cdk_common::mint::MintQuote;
use cdk_common::util::unix_time;
use cdk_common::{Amount, MintQuoteState, PaymentMethod};
use tracing::instrument;

use super::Mint;
use crate::Error;

impl Mint {
    /// Check the status of an ln payment for a quote
    #[instrument(skip_all)]
    pub async fn check_mint_quote_paid(&self, quote: &mut MintQuote) -> Result<(), Error> {
        let state = quote.state();

        // We can just return here and do not need to check with ln node.
        // If quote is issued it is already in a final state,
        // If it is paid ln node will only tell us what we already know
        if quote.payment_method == PaymentMethod::Bolt11
            && (state == MintQuoteState::Issued || state == MintQuoteState::Paid)
        {
            return Ok(());
        }

        let ln = match self.payment_processors.get(&PaymentProcessorKey::new(
            quote.unit.clone(),
            quote.payment_method.clone(),
        )) {
            Some(ln) => ln,
            None => {
                tracing::info!("Could not get ln backend for {}, bolt11 ", quote.unit);

                return Err(Error::UnsupportedUnit);
            }
        };

        let ln_status = ln
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;

        if ln_status.is_empty() {
            return Ok(());
        }

        let mut tx = self.localstore.begin_transaction().await?;

        for payment in ln_status {
            if !quote.payment_ids().contains(&&payment.payment_id)
                && payment.payment_amount > Amount::ZERO
            {
                tracing::debug!(
                    "Found payment of {} {} for quote {} when checking.",
                    payment.payment_amount,
                    payment.unit,
                    quote.id
                );

                let amount_paid = to_unit(payment.payment_amount, &payment.unit, &quote.unit)?;

                quote.increment_amount_paid(amount_paid)?;
                quote.add_payment(amount_paid, payment.payment_id.clone(), unix_time())?;

                let total_paid = tx
                    .increment_mint_quote_amount_paid(&quote.id, amount_paid, payment.payment_id)
                    .await?;

                self.pubsub_manager.mint_quote_payment(quote, total_paid);
            }
        }

        tx.commit().await?;

        Ok(())
    }
}
