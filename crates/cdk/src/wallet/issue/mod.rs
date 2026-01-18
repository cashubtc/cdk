mod bolt11;
mod bolt12;
mod custom;

use cdk_common::PaymentMethod;

use crate::amount::SplitTarget;
use crate::nuts::nut00::KnownMethod;
use crate::nuts::{Proofs, SpendingConditions};
use crate::wallet::MintQuote;
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
}
