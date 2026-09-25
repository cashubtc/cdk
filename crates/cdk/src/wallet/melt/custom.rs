use cdk_common::wallet::MeltQuote;
use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest, PaymentMethod};
use tracing::instrument;

use crate::nuts::MeltQuoteCustomRequest;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Melt Quote for Custom Payment Method
    ///
    /// # Arguments
    /// * `method` - Custom payment method name
    /// * `request` - Payment request string (method-specific format)
    /// * `amount` - Optional amount in wallet currency unit
    /// * `extra` - Optional extra payment-method-specific data as JSON
    #[instrument(skip(self, request, extra))]
    pub(crate) async fn melt_quote_custom(
        &self,
        method: &str,
        request: String,
        amount: Option<Amount>,
        extra: Option<serde_json::Value>,
    ) -> Result<MeltQuote, Error> {
        self.keysets(Default::default()).await?;

        let quote_request = MeltQuoteCustomRequest {
            method: method.to_string(),
            request: request.clone(),
            unit: self.unit.clone(),
            amount,
            extra: extra.unwrap_or(serde_json::Value::Null),
        };
        let quote_res = self
            .client
            .post_melt_quote(MeltQuoteRequest::Custom(quote_request))
            .await?;

        let quote_res = match quote_res {
            MeltQuoteCreateResponse::Custom((_, response)) => response,
            _ => return Err(Error::InvalidPaymentMethod),
        };

        // Construct MeltQuote from custom response
        // Use response's request if present, otherwise fallback to input request
        let quote_request_str = quote_res.request.unwrap_or(request);

        let quote = MeltQuote {
            id: quote_res.quote,
            mint_url: Some(self.mint_url.clone()),
            amount: quote_res.amount,
            request: quote_request_str,
            unit: self.unit.clone(),
            fee_reserve: quote_res.fee_reserve.unwrap_or_default(),
            state: quote_res.state,
            expiry: quote_res.expiry,
            payment_proof: quote_res.payment_preimage,
            estimated_blocks: None,
            fee_index: None,
            payment_method: PaymentMethod::Custom(method.to_string()),

            used_by_operation: None,
            version: 0,
        };

        self.localstore.add_melt_quote(quote.clone()).await?;

        Ok(quote)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::nuts::nut05::QuoteState;
    use cdk_common::nuts::{CurrencyUnit, MeltQuoteCustomResponse};
    use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest};
    use serde_json::json;

    use super::*;
    use crate::mint_url::MintUrl;
    use crate::wallet::saga::test_utils::create_test_db;
    use crate::wallet::test_utils::{test_keyset, MockMintConnector};
    use crate::wallet::WalletBuilder;
    use crate::Amount;

    #[tokio::test]
    async fn test_melt_quote_custom_preserves_custom_unit_amount_without_msat_conversion() {
        let db = create_test_db().await;
        let mock_connector = Arc::new(MockMintConnector::new());
        let mint_url = MintUrl::from_str("https://mint.example.com").expect("valid URL");
        let ora_unit = CurrencyUnit::Custom("ora".into());

        let mut keyset = test_keyset();
        keyset.unit = ora_unit.clone();
        mock_connector.set_active_keyset(keyset);

        let canned_response = MeltQuoteCustomResponse {
            quote: "test-quote-id".to_string(),
            method: PaymentMethod::Custom("branch".to_string()),
            amount: Amount::from(500),
            fee_reserve: Some(Amount::from(10)),
            state: QuoteState::Unpaid,
            expiry: 9999999,
            payment_preimage: None,
            change: None,
            request: Some("test-request".to_string()),
            unit: Some(ora_unit.clone()),
            extra: json!({"foo": "bar"}),
        };

        mock_connector
            .post_melt_quote_responses
            .lock()
            .unwrap()
            .push_back(Ok(MeltQuoteCreateResponse::Custom((
                PaymentMethod::Custom("branch".to_string()),
                canned_response,
            ))));

        let wallet = WalletBuilder::new()
            .mint_url(mint_url)
            .unit(ora_unit.clone())
            .localstore(db)
            .seed([42; 64])
            .shared_client(mock_connector.clone())
            .build()
            .expect("build wallet");

        let quote = wallet
            .melt_quote(
                PaymentMethod::Custom("branch".to_string()),
                "test-request",
                None,
                Some(r#"{"amount": 500, "foo": "bar"}"#.to_string()),
            )
            .await
            .expect("melt quote");

        assert_eq!(quote.amount, Amount::from(500));
        assert_eq!(quote.unit, ora_unit);

        let captured = mock_connector.post_melt_quote_requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            MeltQuoteRequest::Custom(req) => {
                assert_eq!(req.method, "branch");
                assert_eq!(req.unit, ora_unit);
                assert_eq!(req.request, "test-request");
                assert_eq!(req.amount, Some(Amount::from(500)));
                assert_eq!(req.extra, json!({"foo": "bar"}));
            }
            _ => panic!("expected custom request"),
        }
    }

    #[tokio::test]
    async fn test_melt_quote_custom_none_amount_remains_none() {
        let db = create_test_db().await;
        let mock_connector = Arc::new(MockMintConnector::new());
        let mint_url = MintUrl::from_str("https://mint.example.com").expect("valid URL");
        let ora_unit = CurrencyUnit::Custom("ora".into());

        let mut keyset = test_keyset();
        keyset.unit = ora_unit.clone();
        mock_connector.set_active_keyset(keyset);

        let canned_response = MeltQuoteCustomResponse {
            quote: "test-quote-id".to_string(),
            method: PaymentMethod::Custom("branch".to_string()),
            amount: Amount::from(100),
            fee_reserve: Some(Amount::from(0)),
            state: QuoteState::Unpaid,
            expiry: 9999999,
            payment_preimage: None,
            change: None,
            request: Some("test-request".to_string()),
            unit: Some(ora_unit.clone()),
            extra: serde_json::Value::Null,
        };

        mock_connector
            .post_melt_quote_responses
            .lock()
            .unwrap()
            .push_back(Ok(MeltQuoteCreateResponse::Custom((
                PaymentMethod::Custom("branch".to_string()),
                canned_response,
            ))));

        let wallet = WalletBuilder::new()
            .mint_url(mint_url)
            .unit(ora_unit.clone())
            .localstore(db)
            .seed([42; 64])
            .shared_client(mock_connector.clone())
            .build()
            .expect("build wallet");

        let quote = wallet
            .melt_quote(
                PaymentMethod::Custom("branch".to_string()),
                "test-request",
                None,
                Some(r#"{"foo": "bar"}"#.to_string()),
            )
            .await
            .expect("melt quote");

        assert_eq!(quote.amount, Amount::from(100));

        let captured = mock_connector.post_melt_quote_requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            MeltQuoteRequest::Custom(req) => {
                assert_eq!(req.method, "branch");
                assert_eq!(req.unit, ora_unit);
                assert_eq!(req.request, "test-request");
                assert_eq!(req.amount, None);
                assert_eq!(req.extra, json!({"foo": "bar"}));
            }
            _ => panic!("expected custom request"),
        }
    }

    #[tokio::test]
    async fn test_melt_quote_custom_sat_unit_does_not_convert_to_msat() {
        let db = create_test_db().await;
        let mock_connector = Arc::new(MockMintConnector::new());
        let mint_url = MintUrl::from_str("https://mint.example.com").expect("valid URL");

        let canned_response = MeltQuoteCustomResponse {
            quote: "test-quote-id".to_string(),
            method: PaymentMethod::Custom("branch".to_string()),
            amount: Amount::from(500),
            fee_reserve: Some(Amount::from(2)),
            state: QuoteState::Unpaid,
            expiry: 9999999,
            payment_preimage: None,
            change: None,
            request: Some("test-request".to_string()),
            unit: Some(CurrencyUnit::Sat),
            extra: serde_json::Value::Null,
        };

        mock_connector
            .post_melt_quote_responses
            .lock()
            .unwrap()
            .push_back(Ok(MeltQuoteCreateResponse::Custom((
                PaymentMethod::Custom("branch".to_string()),
                canned_response,
            ))));

        let wallet = WalletBuilder::new()
            .mint_url(mint_url)
            .unit(CurrencyUnit::Sat)
            .localstore(db)
            .seed([42; 64])
            .shared_client(mock_connector.clone())
            .build()
            .expect("build wallet");

        wallet
            .melt_quote(
                PaymentMethod::Custom("branch".to_string()),
                "test-request",
                None,
                Some(r#"{"amount": 500}"#.to_string()),
            )
            .await
            .expect("melt quote");

        let captured = mock_connector.post_melt_quote_requests.lock().unwrap();
        assert_eq!(captured.len(), 1);
        match &captured[0] {
            MeltQuoteRequest::Custom(req) => {
                assert_eq!(req.method, "branch");
                assert_eq!(req.unit, CurrencyUnit::Sat);
                assert_eq!(req.amount, Some(Amount::from(500)));
            }
            _ => panic!("expected custom request"),
        }
    }
}
