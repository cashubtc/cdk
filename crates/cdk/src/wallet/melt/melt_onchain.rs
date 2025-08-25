//! Melt Onchain
//!
//! Implementation of melt functionality for onchain Bitcoin transactions

use cdk_common::nut26::MeltQuoteOnchainRequest;
use cdk_common::wallet::MeltQuote;
use tracing::instrument;

use crate::nuts::MeltQuoteOnchainResponse;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for onchain Bitcoin transaction
    #[instrument(skip(self, request))]
    pub async fn melt_onchain_quote(
        &self,
        request: String,
        amount: Amount,
    ) -> Result<MeltQuote, Error> {
        let quote_request = MeltQuoteOnchainRequest {
            request: request.clone(),
            unit: self.unit.clone(),
            amount,
        };

        let quote_res = self.client.post_melt_onchain_quote(quote_request).await?;

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: None, // Onchain transactions don't have preimages like Lightning
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Onchain melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_onchain_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteOnchainResponse<String>, Error> {
        let response = self.client.get_melt_onchain_quote_status(quote_id).await?;

        match self.localstore.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                if let Err(e) = self
                    .add_transaction_for_pending_melt_onchain(&quote, &response)
                    .await
                {
                    tracing::error!("Failed to add transaction for pending melt onchain: {}", e);
                }

                quote.state = response.state;
                self.localstore.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        Ok(response)
    }
}
