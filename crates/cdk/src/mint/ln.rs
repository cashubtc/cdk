use cdk_common::common::PaymentProcessorKey;
use cdk_common::database::{self, MintTransaction};
use cdk_common::mint::MintQuote;
use cdk_common::MintQuoteState;

use super::Mint;
use crate::Error;

impl Mint {
    /// Check the status of an ln payment for a quote
    pub async fn check_mint_quote_paid(
        &self,
        tx: &mut Box<dyn MintTransaction<'_, database::Error> + Send + Sync + '_>,
        quote: &mut MintQuote,
    ) -> Result<(), Error> {
        let ln = match self.ln.get(&PaymentProcessorKey::new(
            quote.unit.clone(),
            cdk_common::PaymentMethod::Bolt11,
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

        if ln_status != quote.state && quote.state != MintQuoteState::Issued {
            tx.update_mint_quote_state(&quote.id, ln_status).await?;

            quote.state = ln_status;

            self.pubsub_manager
                .mint_quote_bolt11_status(quote.clone(), ln_status);
        }

        Ok(())
    }
}
