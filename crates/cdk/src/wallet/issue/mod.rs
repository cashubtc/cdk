//! Issue (Mint) module for the wallet.
//!
//! This module provides functionality for minting new proofs via Bolt11, Bolt12, and Custom methods.

pub(crate) mod saga;

use cdk_common::nut00::KnownMethod;
use cdk_common::nut04::MintMethodOptions;
use cdk_common::nut25::MintQuoteBolt12Request;
use cdk_common::PaymentMethod;
pub(crate) use saga::MintSaga;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::nuts::{
    MintQuoteBolt11Request, MintQuoteCustomRequest, Proofs, SecretKey, SpendingConditions,
};
use crate::util::unix_time;
use crate::wallet::recovery::RecoveryAction;
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
            PaymentMethod::Custom(ref method) => {
                let mint_quote_response = self
                    .client
                    .get_mint_quote_custom_status(method, &mint_quote.id)
                    .await?;

                mint_quote.state = mint_quote_response.state;

                // Update amounts based on state
                match mint_quote_response.state {
                    MintQuoteState::Paid => {
                        mint_quote.amount_paid = mint_quote_response.amount.unwrap_or_default();
                    }
                    MintQuoteState::Issued => {
                        mint_quote.amount_paid = mint_quote_response.amount.unwrap_or_default();
                        mint_quote.amount_issued = mint_quote_response.amount.unwrap_or_default();
                    }
                    MintQuoteState::Unpaid => (),
                }
            }
        }

        Ok(())
    }

    /// This method:
    /// 1. Fetches the current quote state from the mint
    /// 2. If there's an in-progress saga for this quote, attempts to complete it
    /// 3. If the saga was compensated (rolled back), attempts a fresh mint
    /// 4. Returns the updated quote
    #[instrument(skip_all)]
    async fn inner_check_mint_quote_status(
        &self,
        mut mint_quote: MintQuote,
    ) -> Result<MintQuote, Error> {
        let quote_id = mint_quote.id.clone();
        // First, check/update the state from the mint
        self.check_state(&mut mint_quote).await?;

        // Check if there's an in-progress saga for this quote
        if let Some(ref operation_id_str) = mint_quote.used_by_operation {
            if let Ok(operation_id) = uuid::Uuid::parse_str(operation_id_str) {
                match self.localstore.get_saga(&operation_id).await {
                    Ok(Some(saga)) => {
                        // Saga exists - try to complete it (like recovery does)
                        tracing::info!(
                            "Mint quote {} has in-progress saga {}, attempting to complete",
                            quote_id,
                            operation_id
                        );

                        let recovery_action = self.resume_issue_saga(&saga).await?;

                        // If compensated, the saga was rolled back - attempt to mint again
                        if recovery_action == RecoveryAction::Compensated {
                            tracing::info!(
                                "Saga {} was compensated, attempting fresh mint for quote {}",
                                operation_id,
                                quote_id
                            );
                        } else {
                            // If the saga completed we need to get the updated state of the mint quote fn the db
                            mint_quote = self
                                .localstore
                                .get_mint_quote(&quote_id)
                                .await?
                                .ok_or(Error::UnknownQuote)?;
                        }
                        // If Recovered or Skipped, just continue with the updated quote
                    }
                    Ok(None) => {
                        // Orphaned reservation - release it
                        tracing::warn!(
                            "Mint quote {} has orphaned reservation for operation {}, releasing",
                            quote_id,
                            operation_id
                        );
                        if let Err(e) = self.localstore.release_mint_quote(&operation_id).await {
                            tracing::warn!("Failed to release orphaned mint quote: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to check saga for mint quote {}: {}", quote_id, e);
                        return Err(Error::Database(e));
                    }
                }
            }
        }

        self.localstore.add_mint_quote(mint_quote.clone()).await?;
        Ok(mint_quote)
    }

    /// Check the status of a single mint quote from the mint.
    ///
    /// Calls `GET /v1/mint/quote/{method}/{quote_id}` per NUT-04.
    /// Updates local store with current state from mint.
    /// If there was a crashed mid-mint (pending saga), attempts to complete it.
    /// Does NOT mint tokens directly - use mint() for that.
    ///
    /// **Note:** The mint quote must be known to the wallet (stored locally) for this
    /// function to work. If the quote is not stored locally, use `fetch_mint_quote`
    /// instead.
    #[instrument(skip(self, quote_id))]
    pub async fn check_mint_quote_status(&self, quote_id: &str) -> Result<MintQuote, Error> {
        let mint_quote = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        let mint_quote = self.inner_check_mint_quote_status(mint_quote).await?;

        Ok(mint_quote)
    }

    /// Check all unissued mint quote states from the mint.
    ///
    /// Calls `GET /v1/mint/quote/{method}/{quote_id}` per NUT-04 for each quote.
    /// Updates local store with current state from mint for each quote.
    /// If there was a crashed mid-mint (pending saga), attempts to complete it.
    /// Does NOT mint tokens directly - use mint() or mint_unissued_quotes() for that.
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let mint_quotes = self.localstore.get_unissued_mint_quotes().await?;
        let mut updated_quotes = Vec::new();

        for mint_quote in mint_quotes {
            if mint_quote.mint_url != self.mint_url || mint_quote.unit != self.unit {
                continue;
            }

            match self.inner_check_mint_quote_status(mint_quote).await {
                Ok(q) => updated_quotes.push(q),
                Err(err) => {
                    tracing::warn!("Could not check quote state: {}", err);
                    continue;
                }
            }
        }
        Ok(updated_quotes)
    }

    /// Refresh states and mint all unissued quotes that have mintable amounts.
    /// Returns the total amount minted across all quotes.
    ///
    /// # Privacy
    ///
    /// This method retrieves all unissued mint quotes from the local store and
    /// checks their state with the mint. This has a negative privacy effect of
    /// linking all these quotes to a single wallet session.
    #[instrument(skip(self))]
    pub async fn mint_unissued_quotes(&self) -> Result<Amount, Error> {
        let mint_quotes = self.localstore.get_unissued_mint_quotes().await?;
        let mut total_amount = Amount::ZERO;

        for mint_quote in mint_quotes {
            if mint_quote.mint_url != self.mint_url || mint_quote.unit != self.unit {
                continue;
            }

            let current_amount_issued = mint_quote.amount_issued;

            let mint_quote = match self.inner_check_mint_quote_status(mint_quote).await {
                Ok(q) => q,
                Err(err) => {
                    tracing::warn!("Could not check quote state: {}", err);
                    continue;
                }
            };

            if mint_quote.amount_mintable() > Amount::ZERO {
                if let Err(err) = self
                    .mint(&mint_quote.id, SplitTarget::default(), None)
                    .await
                {
                    tracing::warn!("Could not mint quote {}: {}", mint_quote.id, err);
                    continue;
                }
            }

            // Get updated quote to calculate minted amount
            let updated_quote = match self.localstore.get_mint_quote(&mint_quote.id).await {
                Ok(Some(q)) => q,
                _ => continue,
            };

            total_amount = total_amount
                .checked_add(
                    updated_quote
                        .amount_issued
                        .checked_sub(current_amount_issued)
                        .unwrap_or_default(),
                )
                .ok_or(Error::AmountOverflow)?;
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

    /// Fetch a mint quote from the mint and store it locally
    ///
    /// This method contacts the mint to get the current state of a quote,
    /// creates or updates the quote in local storage, and returns the stored quote.
    ///
    /// Works with all payment methods (Bolt11, Bolt12, and custom payment methods).
    ///
    /// # Arguments
    /// * `quote_id` - The ID of the quote to fetch
    /// * `payment_method` - The payment method for the quote. Required if the quote
    ///   is not already stored locally. If the quote exists locally, the stored
    ///   payment method will be used and this parameter is ignored.
    ///
    /// # Errors
    /// Returns `Error::PaymentMethodRequired` if the quote is not found locally
    /// and no payment method is provided.
    #[instrument(skip(self, quote_id))]
    pub async fn fetch_mint_quote(
        &self,
        quote_id: &str,
        payment_method: Option<PaymentMethod>,
    ) -> Result<MintQuote, Error> {
        // Check if we already have this quote stored locally
        let existing_quote = self.localstore.get_mint_quote(quote_id).await?;

        // Determine the payment method to use
        let method = match (&existing_quote, &payment_method) {
            (Some(q), _) => q.payment_method.clone(),
            (None, Some(m)) => m.clone(),
            (None, None) => return Err(Error::PaymentMethodRequired),
        };

        // Fetch the quote status from the mint based on payment method
        let quote = match &method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let response = self.client.get_mint_quote_status(quote_id).await?;

                match existing_quote {
                    Some(mut existing) => {
                        // Update the existing quote with new state
                        existing.state = response.state;
                        existing
                    }
                    None => {
                        // Create a new quote from the response
                        MintQuote::new(
                            quote_id.to_string(),
                            self.mint_url.clone(),
                            method,
                            response.amount,
                            response.unit.unwrap_or(self.unit.clone()),
                            response.request,
                            response.expiry.unwrap_or(0),
                            None,
                        )
                    }
                }
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let response = self.client.get_mint_quote_bolt12_status(quote_id).await?;

                match existing_quote {
                    Some(mut existing) => {
                        // Update the existing quote with new state from bolt12 response
                        existing.amount_paid = response.amount_paid;
                        existing.amount_issued = response.amount_issued;
                        existing
                    }
                    None => {
                        // Create a new quote from the response
                        MintQuote::new(
                            quote_id.to_string(),
                            self.mint_url.clone(),
                            method,
                            response.amount,
                            response.unit,
                            response.request,
                            response.expiry.unwrap_or(0),
                            None,
                        )
                    }
                }
            }
            PaymentMethod::Custom(custom_method) => {
                let response = self
                    .client
                    .get_mint_quote_custom_status(custom_method, quote_id)
                    .await?;

                match existing_quote {
                    Some(mut existing) => {
                        // Update the existing quote with new state
                        existing.amount_paid = response.amount.unwrap_or_default();
                        existing.amount_issued = response.amount.unwrap_or_default();
                        existing
                    }
                    None => {
                        // Create a new quote from the response
                        MintQuote::new(
                            quote_id.to_string(),
                            self.mint_url.clone(),
                            method,
                            response.amount,
                            response.unit.unwrap_or(self.unit.clone()),
                            response.request,
                            response.expiry.unwrap_or(0),
                            None,
                        )
                    }
                }
            }
        };

        // Store the quote
        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }
}
