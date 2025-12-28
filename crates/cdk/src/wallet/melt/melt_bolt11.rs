use std::str::FromStr;

use cdk_common::PaymentMethod;
use lightning_invoice::Bolt11Invoice;
use tracing::instrument;

use crate::amount::to_unit;
use crate::nuts::{CurrencyUnit, MeltOptions, MeltQuoteBolt11Request, MeltQuoteBolt11Response};
use crate::wallet::MeltQuote;
use crate::{Error, Wallet};

impl Wallet {
    /// Melt Quote
    /// # Synopsis
    /// ```rust,no_run
    ///  use std::sync::Arc;
    ///
    ///  use cdk_sqlite::wallet::memory;
    ///  use cdk::nuts::CurrencyUnit;
    ///  use cdk::wallet::Wallet;
    ///  use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///     let bolt11 = "lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq".to_string();
    ///     let quote = wallet.melt_quote(bolt11, None).await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self, request))]
    pub async fn melt_quote(
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

            let amount_quote_unit = to_unit(amount_msat, &CurrencyUnit::Msat, &self.unit)?;

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
            payment_method: PaymentMethod::Bolt11,
            used_by_operation: None,
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

        match self.localstore.get_melt_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                if let Err(e) = self
                    .add_transaction_for_pending_melt(&quote, &response)
                    .await
                {
                    tracing::error!("Failed to add transaction for pending melt: {}", e);
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
