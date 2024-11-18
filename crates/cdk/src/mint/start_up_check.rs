//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use super::{Error, Mint};

impl Mint {
    /// Check the status of all pending mint quotes in the mint db
    /// with all the lighting backends. This check that any payments
    /// received while the mint was offline are accounted for, and the wallet can mint associated ecash
    pub async fn check_pending_mint_quotes(&self) -> Result<(), Error> {
        let mut pending_quotes = self.get_pending_mint_quotes().await?;
        tracing::info!("There are {} pending mint quotes.", pending_quotes.len());
        let mut unpaid_quotes = self.get_unpaid_mint_quotes().await?;
        tracing::info!("There are {} unpaid mint quotes.", unpaid_quotes.len());

        unpaid_quotes.append(&mut pending_quotes);

        for ln in self.ln.values() {
            for quote in unpaid_quotes.iter() {
                tracing::debug!("Checking status of mint quote: {}", quote.id);
                let lookup_id = quote.request_lookup_id.as_str();
                match ln.check_incoming_invoice_status(lookup_id).await {
                    Ok(state) => {
                        if state != quote.state {
                            tracing::trace!("Mint quote status changed: {}", quote.id);
                            self.localstore
                                .update_mint_quote_state(&quote.id, state)
                                .await?;
                        }
                    }

                    Err(err) => {
                        tracing::warn!("Could not check state of pending invoice: {}", lookup_id);
                        tracing::error!("{}", err);
                    }
                }
            }
        }
        Ok(())
    }
}
