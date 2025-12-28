use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut25::MintQuoteBolt12Request;
use cdk_common::SecretKey;
use tracing::instrument;

use super::MintSaga;
use crate::amount::SplitTarget;
use crate::nuts::{MintQuoteBolt12Response, PaymentMethod, Proofs, SpendingConditions};
use crate::wallet::MintQuote;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Bolt12
    #[instrument(skip(self))]
    pub async fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_info = self.load_mint_info().await?;

        let mint_url = self.mint_url.clone();
        let unit = &self.unit;

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let mint_method_settings = mint_info
                .nuts
                .nut04
                .get_settings(
                    unit,
                    &crate::nuts::PaymentMethod::Known(KnownMethod::Bolt12),
                )
                .ok_or(Error::UnsupportedUnit)?;

            match mint_method_settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let mint_request = MintQuoteBolt12Request {
            amount,
            unit: self.unit.clone(),
            description,
            pubkey: secret_key.public_key(),
        };

        let quote_res = self.client.post_mint_bolt12_quote(mint_request).await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Known(KnownMethod::Bolt12),
            amount,
            unit.clone(),
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint bolt12 using the saga pattern
    #[instrument(skip(self))]
    pub async fn mint_bolt12(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let saga = MintSaga::new(self);
        let saga = saga
            .prepare_bolt12(quote_id, amount, amount_split_target, spending_conditions)
            .await?;
        let saga = saga.execute().await?;

        Ok(saga.into_proofs())
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_bolt12_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let response = self.client.get_mint_quote_bolt12_status(quote_id).await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;
                quote.amount_issued = response.amount_issued;
                quote.amount_paid = response.amount_paid;

                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }
}
