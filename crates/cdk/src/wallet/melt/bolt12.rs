//! Melt BOLT12
//!
//! Implementation of melt functionality for BOLT12 offers

use std::str::FromStr;

use cdk_common::amount::amount_for_offer;
use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::MeltQuote;
use cdk_common::PaymentMethod;
use lightning::offers::offer::Offer;
use tracing::instrument;

use crate::nuts::{CurrencyUnit, MeltOptions, MeltQuoteBolt12Request};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for BOLT12 offer
    #[instrument(skip(self, request))]
    pub(crate) async fn melt_bolt12_quote(
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
            let amount_quote_unit = Amount::new(amount_msat.into(), CurrencyUnit::Msat)
                .convert_to(&self.unit)?
                .into();

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
            payment_method: PaymentMethod::Known(KnownMethod::Bolt12),
            used_by_operation: None,
            version: 0,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}
