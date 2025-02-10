use cdk_common::common::LnKey;
use cdk_common::MintQuoteState;

use super::Mint;
use crate::mint::Uuid;
use crate::Error;

impl Mint {
    /// Check the status of an ln payment for a quote
    pub async fn check_mint_quote_paid(&self, quote_id: &Uuid) -> Result<MintQuoteState, Error> {
        let mut quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let ln = match self.ln.get(&LnKey::new(
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
            .check_incoming_invoice_status(&quote.request_lookup_id)
            .await?;

        if ln_status != quote.state && quote.state != MintQuoteState::Issued {
            self.localstore
                .update_mint_quote_state(quote_id, ln_status)
                .await?;

            quote.state = ln_status;

            self.pubsub_manager
                .mint_quote_bolt11_status(quote.clone(), ln_status);
        }

        Ok(quote.state)
    }
}
