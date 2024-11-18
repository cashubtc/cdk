//! Check used at mint start up
//!
//! These checks are need in the case the mint was offline and the lightning node was node.
//! These ensure that the status of the mint or melt quote matches in the mint db and on the node.

use super::{Error, Mint};
use crate::mint::{MeltQuote, MeltQuoteState, PaymentMethod};
use crate::types::LnKey;

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
            let melt_request_ln_key = self.localstore.get_melt_request(&pending_quote.id).await?;

            let (melt_request, ln_key) = match melt_request_ln_key {
                None => {
                    let ln_key = LnKey {
                        unit: pending_quote.unit,
                        method: PaymentMethod::Bolt11,
                    };

                    (None, ln_key)
                }
                Some((melt_request, ln_key)) => (Some(melt_request), ln_key),
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

            match melt_request {
                Some(melt_request) => {
                    match pay_invoice_response.status {
                        MeltQuoteState::Paid => {
                            if let Err(err) = self
                                .process_melt_request(
                                    &melt_request,
                                    pay_invoice_response.payment_preimage,
                                    pay_invoice_response.total_spent,
                                )
                                .await
                            {
                                tracing::error!(
                                    "Could not process melt request for pending quote: {}",
                                    melt_request.quote
                                );
                                tracing::error!("{}", err);
                            }
                        }
                        MeltQuoteState::Unpaid
                        | MeltQuoteState::Unknown
                        | MeltQuoteState::Failed => {
                            // Payment has not been made we want to unset
                            tracing::info!(
                                "Lightning payment for quote {} failed.",
                                pending_quote.id
                            );
                            if let Err(err) = self.process_unpaid_melt(&melt_request).await {
                                tracing::error!("Could not reset melt quote state: {}", err);
                            }
                        }
                        MeltQuoteState::Pending => {
                            tracing::warn!(
                                "LN payment pending, proofs are stuck as pending for quote: {}",
                                melt_request.quote
                            );
                            // Quote is still pending we do not want to do anything
                            // continue to check next quote
                        }
                    }
                }
                None => {
                    tracing::warn!(
                        "There is no stored melt request for pending melt quote: {}",
                        pending_quote.id
                    );

                    self.localstore
                        .update_melt_quote_state(&pending_quote.id, pay_invoice_response.status)
                        .await?;
                }
            };
        }
        Ok(())
    }
}
