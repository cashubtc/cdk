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
    MintQuoteBolt11Request, MintQuoteCustomRequest, Proofs, SecretKey, SpendingConditions,
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

    /// Checks the state of a mint quote with the mint
    async fn check_state(&self, mint_quote: &mut MintQuote) -> Result<(), Error> {
        match mint_quote.payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let mint_quote_response = self.client.get_mint_quote_status(&mint_quote.id).await?;
                mint_quote.state = mint_quote_response.state;

                match mint_quote_response.state {
                    MintQuoteState::Paid => {
                        mint_quote.amount_paid = mint_quote.amount.unwrap_or_default();
                    }
                    MintQuoteState::Issued => {
                        mint_quote.amount_paid = mint_quote.amount.unwrap_or_default();
                        mint_quote.amount_issued = mint_quote.amount.unwrap_or_default();
                    }
                    MintQuoteState::Unpaid => (),
                }
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let mint_quote_response = self
                    .client
                    .get_mint_quote_bolt12_status(&mint_quote.id)
                    .await?;

                mint_quote.amount_issued = mint_quote_response.amount_issued;
                mint_quote.amount_paid = mint_quote_response.amount_paid;
            }
            PaymentMethod::Custom(ref _method) => {
                tracing::warn!("We cannot check unknown types");
                return Err(Error::UnsupportedPaymentMethod);
            }
        }

        Ok(())
    }

    /// Check mint quote status
    #[instrument(skip(self, quote_id))]
    pub async fn mint_quote_state(&self, quote_id: &str) -> Result<MintQuote, Error> {
        let mut mint_quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        self.check_state(&mut mint_quote).await?;

        match self.localstore.add_mint_quote(mint_quote.clone()).await {
            Ok(_) => (),
            Err(e) => {
                let is_concurrent = matches!(e, cdk_common::database::Error::ConcurrentUpdate);
                if is_concurrent {
                    tracing::debug!(
                        "Concurrent update detected for mint quote {}, retrying",
                        quote_id
                    );
                    let mut fresh_quote = self
                        .localstore
                        .get_mint_quote(quote_id)
                        .await?
                        .ok_or(Error::UnknownQuote)?;

                    self.check_state(&mut fresh_quote).await?;

                    match self.localstore.add_mint_quote(fresh_quote.clone()).await {
                        Ok(_) => (),
                        Err(e) => {
                            if matches!(e, cdk_common::database::Error::ConcurrentUpdate) {
                                return Err(Error::ConcurrentUpdate);
                            }
                            return Err(Error::Database(e));
                        }
                    }
                    mint_quote = fresh_quote;
                } else {
                    return Err(Error::Database(e));
                }
            }
        }

        Ok(mint_quote)
    }

    /// Check status of pending mint quotes
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_unissued_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            let mint_quote = match self.mint_quote_state(&mint_quote.id).await {
                Ok(q) => q,
                Err(err) => {
                    tracing::warn!("Could not check quote state: {}", err);
                    continue;
                }
            };

            let amount_mintable = mint_quote.amount_mintable();

            if amount_mintable > Amount::ZERO {
                let proofs = self
                    .mint(&mint_quote.id, SplitTarget::default(), None)
                    .await?;
                total_amount += proofs.total_amount()?;
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
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        self.refresh_keysets().await?;

        let saga = MintSaga::new(self);
        let saga = saga
            .prepare(quote_id, amount_split_target, spending_conditions)
            .await?;
        let saga = saga.execute().await?;

        Ok(saga.into_proofs())
    }
}
