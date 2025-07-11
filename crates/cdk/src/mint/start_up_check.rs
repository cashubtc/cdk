//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use super::{Error, Mint};
use crate::mint::{MeltQuote, MeltQuoteState, PaymentMethod};
use crate::types::PaymentProcessorKey;

impl Mint {
    /// Checks the states of melt quotes that are **PENDING** or **UNKNOWN** to the mint with the ln node
    pub async fn check_pending_melt_quotes(&self) -> Result<(), Error> {
        let melt_quotes = self.localstore.get_melt_quotes().await?;
        let pending_quotes: Vec<MeltQuote> = melt_quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending || q.state == MeltQuoteState::Unknown)
            .collect();
        tracing::info!("There are {} pending melt quotes.", pending_quotes.len());

        let mut tx = self.localstore.begin_transaction().await?;

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

            if let Err(err) = tx
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

        tx.commit().await?;

        Ok(())
    }
}
