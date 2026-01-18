use cdk_common::wallet::MintQuote;
use cdk_common::SecretKey;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::nuts::{MintQuoteCustomRequest, PaymentMethod, SpendingConditions};
use crate::wallet::issue::MintSaga;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Mint Quote for Custom Payment Method
    #[instrument(skip(self))]
    pub(super) async fn mint_quote_custom(
        &self,
        amount: Option<Amount>,
        method: &PaymentMethod,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<MintQuote, Error> {
        let mint_url = self.mint_url.clone();
        let unit = &self.unit;

        self.refresh_keysets().await?;

        let secret_key = SecretKey::generate();

        let amount = amount.ok_or(Error::AmountUndefined)?;

        let mint_request = MintQuoteCustomRequest {
            amount,
            unit: self.unit.clone(),
            description,
            pubkey: Some(secret_key.public_key()),
            extra: serde_json::from_str(&extra.unwrap_or_default())?,
        };

        let quote_res = self
            .client
            .post_mint_custom_quote(method, mint_request)
            .await?;

        let quote = MintQuote::new(
            quote_res.quote,
            mint_url,
            PaymentMethod::Custom(method.to_string()),
            Some(amount),
            unit.clone(),
            quote_res.request,
            quote_res.expiry.unwrap_or(0),
            Some(secret_key),
        );
        self.localstore.add_mint_quote(quote.clone()).await?;

        Ok(quote)
    }

    /// Mint with custom payment method
    /// This is used for all custom payment methods - delegates to existing mint logic
    #[instrument(skip(self))]
    pub(super) async fn mint_custom(
        &self,
        quote_id: &str,
        amount_split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<cdk_common::Proofs, Error> {
        self.refresh_keysets().await?;

        let saga = MintSaga::new(self);
        let prepared = saga
            .prepare_custom(quote_id, amount_split_target, spending_conditions)
            .await?;

        let finalized = prepared.execute().await?;

        Ok(finalized.into_proofs())
    }
}
