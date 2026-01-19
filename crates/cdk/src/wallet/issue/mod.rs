//! Issue (Mint) module for the wallet.
//!
//! This module provides functionality for minting new proofs via Bolt11, Bolt12, and Custom methods.

pub(crate) mod saga;

use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut25::MintQuoteBolt12Request;
use cdk_common::{PaymentMethod, ProofsMethods};
pub(crate) use saga::MintSaga;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::nuts::{
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteBolt12Response,
    MintQuoteCustomRequest, Proofs, SecretKey, SpendingConditions,
};
use crate::util::unix_time;
use crate::wallet::{MintQuote, MintQuoteState};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote
    #[instrument(skip(self, method))]
    pub async fn mint_quote<T>(
        &self,
        method: T,
        amount: Option<Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Error>
    where
        T: Into<PaymentMethod>,
    {
        let mint_info = self.load_mint_info().await?;
        let mint_url = self.mint_url.clone();
        let unit = self.unit.clone();

        let method: PaymentMethod = method.into();

        // Check settings and description support
        if description.is_some() {
            let settings = mint_info
                .nuts
                .nut04
                .get_settings(&unit, &method)
                .ok_or(Error::UnsupportedUnit)?;

            match settings.options {
                Some(MintMethodOptions::Bolt11 { description }) if description => (),
                _ => return Err(Error::InvoiceDescriptionUnsupported),
            }
        }

        self.refresh_keysets().await?;

        let secret_key = SecretKey::generate();

        let (quote_id, request_str, expiry) = match &method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let amount = amount.ok_or(Error::AmountUndefined)?;
                let request = MintQuoteBolt11Request {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: Some(secret_key.public_key()),
                };

                let response = self.client.post_mint_quote(request).await?;
                (response.quote, response.request, response.expiry)
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let request = MintQuoteBolt12Request {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: secret_key.public_key(),
                };

                let response = self.client.post_mint_bolt12_quote(request).await?;
                (response.quote, response.request, response.expiry)
            }
            PaymentMethod::Custom(_) => {
                let amount = amount.ok_or(Error::AmountUndefined)?;
                let request = MintQuoteCustomRequest {
                    amount,
                    unit: unit.clone(),
                    description,
                    pubkey: Some(secret_key.public_key()),
                    extra: serde_json::from_str(&extra.unwrap_or_default())?,
                };

                let response = self.client.post_mint_custom_quote(&method, request).await?;
                (response.quote, response.request, response.expiry)
            }
        };

        let quote = MintQuote::new(
            quote_id,
            mint_url,
            method.clone(),
            amount,
            unit,
            request_str,
            expiry.unwrap_or(0),
            Some(secret_key),
        );

        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint Bolt11 Quote (Legacy Wrapper)
    pub async fn mint_bolt11_quote(
        &self,
        amount: Amount,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        self.mint_quote(PaymentMethod::BOLT11, Some(amount), description, None)
            .await
    }

    /// Mint Bolt12 Quote
    pub async fn mint_bolt12_quote(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<MintQuote, Error> {
        self.mint_quote(PaymentMethod::BOLT12, amount, description, None)
            .await
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

    /// Check mint bolt12 quote status
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
                            .mint(&mint_quote.id, None, SplitTarget::default(), None)
                            .await?;
                        total_amount += proofs.total_amount()?;
                    }
                }
                PaymentMethod::Known(KnownMethod::Bolt12) => {
                    let mint_quote_response = self.mint_bolt12_quote_state(&mint_quote.id).await?;
                    if mint_quote_response.amount_paid > mint_quote_response.amount_issued {
                        let proofs = self
                            .mint(&mint_quote.id, None, SplitTarget::default(), None)
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

    /// Mint
    #[instrument(skip(self))]
    pub async fn mint(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.refresh_keysets().await?;

        let saga = MintSaga::new(self);
        let saga = saga
            .prepare(quote_id, amount, amount_split_target, spending_conditions)
            .await?;
        let saga = saga.execute().await?;

        Ok(saga.into_proofs())
    }

    /// Mint Bolt12 (Legacy Wrapper)
    #[instrument(skip(self))]
    pub async fn mint_bolt12(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.mint(quote_id, amount, amount_split_target, spending_conditions)
            .await
    }

    /// Mint Custom (Legacy Wrapper)
    #[instrument(skip(self))]
    pub async fn mint_custom(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.mint(quote_id, None, amount_split_target, spending_conditions)
            .await
    }
}
