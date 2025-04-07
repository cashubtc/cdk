use cdk_common::amount::to_unit;
use cdk_common::common::PaymentProcessorKey;
use cdk_common::MintQuoteState;
use tracing::instrument;

use super::Mint;
use crate::mint::Uuid;
use crate::Error;

impl Mint {
    /// Check the status of an ln payment for a quote
    #[instrument(skip(self))]
    pub async fn check_mint_quote_paid(&self, quote_id: &Uuid) -> Result<MintQuoteState, Error> {
        let quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let ln = match self.ln.get(&PaymentProcessorKey::new(
            quote.unit.clone(),
            quote.payment_method.clone(),
        )) {
            Some(ln) => ln,
            None => {
                tracing::info!(
                    "Could not get ln backend for {}, {} ",
                    quote.unit,
                    quote.payment_method
                );

                return Err(Error::UnsupportedUnit);
            }
        };

        let ln_status = ln
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;

        let mut status_updated = false;

        for payment in ln_status {
            if !quote.payment_ids.contains(&payment.payment_id) {
                status_updated = true;
                let amount_paid = to_unit(payment.payment_amount, &payment.unit, &quote.unit)?;
                self.localstore
                    .increment_mint_quote_amount_paid(&quote.id, amount_paid, payment.payment_id)
                    .await?;
                self.pubsub_manager
                    .mint_quote_bolt11_status(quote.clone(), MintQuoteState::Paid);
            }
        }

        let current_state = if status_updated {
            tracing::info!(
                "Stored quote state {} did not match ln state on check.",
                quote.state(),
            );

            self.localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?
                .state()
        } else {
            quote.state()
        };

        Ok(current_state)
    }
}
