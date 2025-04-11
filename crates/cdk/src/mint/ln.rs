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

        // TODO: Here we need to get the payment proof and check if its a payment we've seen before
        let ln_status = ln
            .check_incoming_payment_status(&quote.request_lookup_id)
            .await?;

        if ln_status != quote.state() && quote.state() != MintQuoteState::Issued {
            self.pubsub_manager
                .mint_quote_bolt11_status(quote.clone(), ln_status);
        }

        Ok(ln_status)
    }
}
