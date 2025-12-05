//! Melt BOLT12
//!
//! Implementation of melt functionality for BOLT12 offers

use std::str::FromStr;

use cdk_common::amount::amount_for_offer;
use cdk_common::wallet::MeltQuote;
use cdk_common::PaymentMethod;
use lightning::offers::offer::Offer;
use tracing::instrument;

use crate::amount::to_unit;
use crate::nuts::{CurrencyUnit, MeltOptions, MeltQuoteBolt11Response, MeltQuoteBolt12Request};
use crate::{Error, Wallet};

impl Wallet {
    /// Melt Quote for BOLT12 offer
    #[instrument(skip(self, request))]
    pub async fn melt_bolt12_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, Error> {
        let quote_request = MeltQuoteBolt12Request {
            request: request.clone(),
            unit: self.unit.clone(),
            options,
        };

        let quote_res = self.client.post_melt_bolt12_quote(quote_request).await?;

        if self.unit == CurrencyUnit::Sat || self.unit == CurrencyUnit::Msat {
            let offer = Offer::from_str(&request).map_err(|_| Error::Bolt12parse)?;
            // Get amount from offer or options
            let amount_msat = options
                .map(|opt| opt.amount_msat())
                .or_else(|| amount_for_offer(&offer, &CurrencyUnit::Msat).ok())
                .ok_or(Error::AmountUndefined)?;
            let amount_quote_unit = to_unit(amount_msat, &CurrencyUnit::Msat, &self.unit).unwrap();

            if quote_res.amount != amount_quote_unit {
                tracing::warn!(
                    "Mint returned incorrect quote amount. Expected {}, got {}",
                    amount_quote_unit,
                    quote_res.amount
                );
                return Err(Error::IncorrectQuoteAmount);
            }
        }

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
            payment_method: PaymentMethod::Bolt12,
        };

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.add_melt_quote(quote.clone()).await?;
        tx.commit().await?;

        Ok(quote)
    }

    /// BOLT12 melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_bolt12_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let response = self.client.get_melt_bolt12_quote_status(quote_id).await?;

        let mut tx = self.localstore.begin_db_transaction().await?;

        match tx.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                if let Err(e) = self
                    .add_transaction_for_pending_melt(&mut tx, &quote, &response)
                    .await
                {
                    tracing::error!("Failed to add transaction for pending melt: {}", e);
                }

                quote.state = response.state;
                tx.add_melt_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote melt {} unknown", quote_id);
            }
        }

        tx.commit().await?;

        Ok(response)
    }
}
