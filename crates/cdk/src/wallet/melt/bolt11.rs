use std::str::FromStr;

use cdk_common::nut00::KnownMethod;
use cdk_common::wallet::MeltQuote;
use cdk_common::PaymentMethod;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;

use crate::nuts::{CurrencyUnit, MeltOptions, MeltQuoteBolt11Request, MeltQuoteBolt11Response};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for Bolt11
    #[instrument(skip(self, request))]
    pub(crate) async fn melt_bolt11_quote(
        &self,
        request: String,
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, Error> {
        let invoice = Bolt11Invoice::from_str(&request)?;

        let quote_request = MeltQuoteBolt11Request {
            request: invoice.clone(),
            unit: self.unit.clone(),
            options,
        };

        let quote_res = self.client.post_melt_quote(quote_request).await?;

        if self.unit == CurrencyUnit::Msat || self.unit == CurrencyUnit::Sat {
            let amount_msat = options
                .map(|opt| opt.amount_msat().into())
                .or_else(|| invoice.amount_milli_satoshis())
                .ok_or(Error::InvoiceAmountUndefined)?;

            let amount_quote_unit = Amount::new(amount_msat, CurrencyUnit::Msat)
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

        // Construct MeltQuote from response
        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            used_by_operation: None,
            version: 0,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let response = self.client.get_melt_quote_status(quote_id).await?;

        if let Some(mut quote) = self.localstore.get_melt_quote(quote_id).await? {
            self.update_melt_quote_state(
                &mut quote,
                response.state,
                response.amount,
                response.change_amount(),
                response.payment_preimage.clone(),
            )
            .await?;
        } else {
            tracing::info!("Quote melt {} unknown", quote_id);
        }

        Ok(response)
    }
}
