use cdk_common::wallet::MeltQuote;
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
        options: Option<MeltOptions>,
    ) -> Result<MeltQuote, Error> {
        self.refresh_keysets().await?;

        let _quote_request = MeltQuoteCustomRequest {
            method: method.to_string(),
            request: request.clone(),
            unit: self.unit.clone(),
            data: serde_json::Value::Null, // Can be extended for method-specific data
        };

        // For now, we'll use the bolt11 quote endpoint until custom endpoints are added
        // This is a temporary workaround - will be replaced in Phase 3
        // let quote_res = self.client.post_melt_custom_quote(quote_request).await?;

        // TODO: Once custom HTTP client methods are implemented, use them instead
        // For now, return an error indicating custom methods need HTTP support
        return Err(Error::UnsupportedPaymentMethod);

        // The following code will be enabled once HTTP client methods exist:
        /*
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
        */
    }
}
