use cdk_common::wallet::MeltQuote;
use cdk_common::PaymentMethod;
use tracing::instrument;

use crate::nuts::{MeltOptions, MeltQuoteCustomRequest};
use crate::{Error, Wallet};

impl Wallet {
    /// Melt Quote for Custom Payment Method
    ///
    /// # Arguments
    /// * `method` - Custom payment method name
    /// * `request` - Payment request string (method-specific format)
    /// * `_options` - Melt options (currently unused for custom methods)
    /// * `extra` - Optional extra payment-method-specific data as JSON
    #[instrument(skip(self, request, extra))]
    pub(super) async fn melt_quote_custom(
        &self,
        method: &str,
        request: String,
        _options: Option<MeltOptions>,
        extra: Option<serde_json::Value>,
    ) -> Result<MeltQuote, Error> {
        self.refresh_keysets().await?;

        let quote_request = MeltQuoteCustomRequest {
            method: method.to_string(),
            request: request.clone(),
            unit: self.unit.clone(),
            extra: extra.unwrap_or(serde_json::Value::Null),
        };
        let quote_res = self.client.post_melt_custom_quote(quote_request).await?;

        // Construct MeltQuote from custom response
        // Use response's request if present, otherwise fallback to input request
        let quote_request_str = quote_res.request.unwrap_or(request);

        let quote = MeltQuote {
            id: quote_res.quote,
            amount: quote_res.amount,
            request: quote_request_str,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve,
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_preimage: quote_res.payment_preimage,
            payment_method: PaymentMethod::Custom(method.to_string()),
            used_by_operation: None,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}
