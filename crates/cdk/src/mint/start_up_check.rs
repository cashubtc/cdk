//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use super::{Error, Mint};
use crate::mint::{MeltQuote, MeltQuoteState, PaymentMethod};
use crate::types::PaymentProcessorKey;

impl Mint {
    /// Check the status of all pending and unpaid mint quotes in the mint db
    /// with all the lighting backends. This check that any payments
    /// received while the mint was offline are accounted for, and the wallet can mint associated ecash
    pub async fn check_pending_mint_quotes(&self) -> Result<(), Error> {
        let pending_quotes = self.get_pending_mint_quotes().await?;
        let unpaid_quotes = self.get_unpaid_mint_quotes().await?;

        let all_quotes = vec![pending_quotes, unpaid_quotes].concat();

        tracing::info!(
            "There are {} pending and unpaid mint quotes.",
            all_quotes.len()
        );
        for quote in all_quotes.iter() {
            tracing::debug!("Checking status of mint quote: {}", quote.id);
            if let Err(err) = self.check_mint_quote_paid(&quote.id).await {
                tracing::error!("Could not check status of {}, {}", quote.id, err);
            }
        }
        Ok(())
    }

    /// Checks the states of melt quotes that are **PENDING** or **UNKNOWN** to the mint with the ln node
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        let pending_quotes: Vec<MeltQuote> = melt_quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending || q.state == MeltQuoteState::Unknown)
            .collect();
        tracing::info!("There are {} pending melt quotes.", pending_quotes.len());

        for pending_quote in pending_quotes {
            tracing::debug!("Checking status for melt quote {}.", pending_quote.id);

            let ln_key = PaymentProcessorKey {
                unit: pending_quote.unit,
                method: PaymentMethod::Bolt11,
            };

            let ln_backend = match self.ln.get(&ln_key) {
                Some(ln_backend) => ln_backend,
                None => {
                    tracing::warn!("No backend for ln key: {:?}", ln_key);
                    continue;
                }
            };

            let pay_invoice_response = ln_backend
                .check_outgoing_payment(&pending_quote.request_lookup_id)
                .await?;

            tracing::warn!(
                "There is no stored melt request for pending melt quote: {}",
                pending_quote.id
            );

            let melt_quote_state = match pay_invoice_response.status {
                MeltQuoteState::Unpaid => MeltQuoteState::Unpaid,
                MeltQuoteState::Paid => MeltQuoteState::Paid,
                MeltQuoteState::Pending => MeltQuoteState::Pending,
                MeltQuoteState::Failed => MeltQuoteState::Unpaid,
                MeltQuoteState::Unknown => MeltQuoteState::Unpaid,
            };

            if let Err(err) = self
                .localstore
                .update_melt_quote_state(&pending_quote.id, melt_quote_state)
                .await
            {
                tracing::error!(
                    "Could not update quote {} to state {}, current state {}, {}",
                    pending_quote.id,
                    melt_quote_state,
                    pending_quote.state,
                    err
                );
            };
        }
        Ok(())
    }
}
