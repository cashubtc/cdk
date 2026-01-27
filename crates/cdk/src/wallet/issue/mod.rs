mod bolt11;
mod bolt12;
mod custom;

use cdk_common::wallet::MintQuote;
use cdk_common::PaymentMethod;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::nuts::nut00::KnownMethod;
use crate::nuts::{Proofs, SpendingConditions};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Unified mint quote method for all payment methods
    /// Routes to the appropriate handler based on the payment method
    pub async fn mint_quote_unified(
        &self,
        amount: Option<Amount>,
        method: PaymentMethod,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Error> {
        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                // For bolt11, request should be empty or ignored, amount is required
                let amount = amount.ok_or(Error::AmountUndefined)?;
                self.mint_quote(amount, description).await
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                // For bolt12, request is the offer string
                self.mint_bolt12_quote(amount, description).await
            }
            PaymentMethod::Custom(ref _custom_method) => {
                self.mint_quote_custom(amount, &method, description, extra)
                    .await
            }
        }
    }

    /// Unified mint method for all payment methods
    /// Routes to the appropriate handler based on the payment method stored in the quote
    pub async fn mint_unified(
        &self,
        quote_id: &str,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Error> {
        // Fetch the quote to determine the payment method
        let quote_info = self
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        match quote_info.payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                // Bolt11 doesn't need amount parameter
                self.mint(quote_id, amount_split_target, spending_conditions)
                    .await
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                self.mint_bolt12(quote_id, amount, amount_split_target, spending_conditions)
                    .await
            }
            PaymentMethod::Custom(_) => {
                self.mint_custom(quote_id, amount_split_target, spending_conditions)
                    .await
            }
        }
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
