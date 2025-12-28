use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::wallet::MintQuote;
use cdk_common::PaymentMethod;
use tracing::instrument;

use super::MintSaga;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    MintQuoteBolt11Request, MintQuoteBolt11Response, Proofs, SecretKey, SpendingConditions,
};
use crate::util::unix_time;
use crate::wallet::MintQuoteState;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use cdk::amount::Amount;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_info = self.load_mint_info().await?;

        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        // If we have a description, we check that the mint supports it.
        if description.is_some() {
            let settings = mint_info
                .nuts
                .nut04
                .get_settings(
                    &unit,
                    &crate::nuts::PaymentMethod::Known(KnownMethod::Bolt11),
                )
                .ok_or(Error::UnsupportedUnit)?;

            match settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        let secret_key = SecretKey::generate();

        let request = MintQuoteBolt11Request {
            amount,
            unit: unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
        };

        let quote_res = self.client.post_mint_quote(request).await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Known(KnownMethod::Bolt11),
            Some(amount),
            unit,
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let response = self.client.get_mint_quote_status(quote_id).await?;

        match self.localstore.get_mint_quote(quote_id).await? {
            Some(quote) => {
                let mut quote = quote;

                quote.state = response.state;
                self.localstore.add_mint_quote(quote).await?;
            }
            None => {
                tracing::info!("Quote mint {} unknown", quote_id);
            }
        }

        Ok(response)
    }

    /// Check status of pending mint quotes
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_unissued_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            match mint_quote.payment_method {
                PaymentMethod::Known(KnownMethod::Bolt11) => {
                    let mint_quote_response = self.mint_quote_state(&mint_quote.id).await?;

                    if mint_quote_response.state == MintQuoteState::Paid {
                        let proofs = self
                            .mint(&mint_quote.id, SplitTarget::default(), None)
                            .await?;
                        total_amount += proofs.total_amount()?;
                    }
                }
                PaymentMethod::Known(KnownMethod::Bolt12) => {
                    let mint_quote_response = self.mint_bolt12_quote_state(&mint_quote.id).await?;
                    if mint_quote_response.amount_paid > mint_quote_response.amount_issued {
                        let proofs = self
                            .mint_bolt12(&mint_quote.id, None, SplitTarget::default(), None)
                            .await?;
                        total_amount += proofs.total_amount()?;
                    }
                }
                PaymentMethod::Custom(_) => {
                    tracing::warn!("We cannot check unknown types");
                }
            }
        }
        Ok(total_amount)
    }

    /// Get active mint quotes
    /// Returns mint quotes that are not expired and not yet issued.
    #[instrument(skip(self))]
    pub async fn get_active_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mut mint_quotes = self.localstore.get_mint_quotes().await?;
        let unix_time = unix_time();
        mint_quotes.retain(|quote| {
            quote.mint_url == self.mint_url
                && quote.state != MintQuoteState::Issued
                && quote.expiry > unix_time
        });
        Ok(mint_quotes)
    }

    /// Get unissued mint quotes
    /// Returns bolt11 quotes where nothing has been issued yet (amount_issued = 0) and all bolt12 quotes.
    /// Includes unpaid bolt11 quotes to allow checking with the mint if they've been paid (wallet state may be outdated).
    /// Filters out quotes from other mints. Does not filter by expiry time to allow
    /// checking with the mint if expired quotes can still be minted.
    #[instrument(skip(self))]
    pub async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mut pending_quotes = self.localstore.get_unissued_mint_quotes().await?;
        pending_quotes.retain(|quote| quote.mint_url == self.mint_url);
        Ok(pending_quotes)
    }

    /// Mint using the saga pattern
    /// # Synopsis
    /// ```rust,no_run
    /// use std::sync::Arc;
    ///
    /// use anyhow::Result;
    /// use cdk::amount::{Amount, SplitTarget};
    /// use cdk::nuts::nut00::ProofsMethods;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use cdk_sqlite::wallet::memory;
    /// use rand::random;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let seed = random::<[u8; 64]>();
    ///     let mint_url = "https://fake.thesimplekid.dev";
    ///     let unit = CurrencyUnit::Sat;
    ///
    ///     let localstore = memory::empty().await?;
    ///     let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();
    ///     let amount = Amount::from(100);
    ///
    ///     let quote = wallet.mint_quote(amount, None).await?;
    ///     let quote_id = quote.id;
    ///     // To be called after quote request is paid
    ///     let minted_proofs = wallet.mint(&quote_id, SplitTarget::default(), None).await?;
    ///     let minted_amount = minted_proofs.total_amount()?;
    ///
    ///     Ok(())
    /// }
    /// ```
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        let saga = MintSaga::new(self);
        let saga = saga
            .prepare_bolt11(quote_id, amount_split_target, spending_conditions)
            .await?;
        let saga = saga.execute().await?;

        Ok(saga.into_proofs())
    }
}
