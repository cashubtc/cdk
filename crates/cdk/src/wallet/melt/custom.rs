use cdk_common::wallet::MeltQuote;
use cdk_common::PaymentMethod;
use tracing::instrument;

use crate::nuts::{MeltOptions, MeltQuoteCustomRequest};
use crate::{Error, Wallet};

impl Wallet {
    /// Melt Quote for Custom Payment Method
    #[instrument(skip(self, request))]
    pub(super) async fn melt_quote_custom(
        &self,
        method: &str,
        request: String,
        _options: Option<MeltOptions>,
    ) -> Result<MeltQuote, Error> {
        self.refresh_keysets().await?;

        let quote_request = MeltQuoteCustomRequest {
            method: method.to_string(),
            request: request.clone(),
            unit: self.unit.clone(),
            extra: serde_json::Value::Null,
        };
        let quote_res = self.client.post_melt_custom_quote(quote_request).await?;

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
            payment_method: PaymentMethod::Custom(method.to_string()),
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}
