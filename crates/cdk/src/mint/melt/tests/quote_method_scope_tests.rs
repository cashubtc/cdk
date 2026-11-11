//! Tests that a melt quote request cannot name a method other than the
//! route's, which only a custom request can even express.

use cdk_common::melt::MeltQuoteRequest;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteCustomRequest};
use cdk_common::PaymentMethod;

use crate::mint::MintServer;
use crate::test_helpers::mint::create_test_mint;
use crate::Error;

fn custom_request(method: &str) -> MeltQuoteRequest {
    MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
        method: method.to_string(),
        request: "pay-me".to_string(),
        unit: CurrencyUnit::Sat,
        amount: Some(1.into()),
        extra: serde_json::Value::Null,
    })
}

#[tokio::test]
async fn melt_quote_rejects_a_request_naming_another_method() {
    let mint = create_test_mint().await.expect("test mint");

    let error = mint
        .post_melt_quote(&PaymentMethod::from("paypal"), custom_request("venmo"))
        .await
        .expect_err("a venmo request on the paypal route must be rejected");

    assert!(
        matches!(error, Error::InvalidPaymentMethod),
        "expected InvalidPaymentMethod, got {error:?}"
    );
    assert!(
        mint.melt_quotes().await.expect("melt quotes").is_empty(),
        "the rejected request must not have created a quote"
    );
}

#[tokio::test]
async fn melt_quote_accepts_a_method_differing_only_in_case() {
    let mint = create_test_mint().await.expect("test mint");

    let error = mint
        .post_melt_quote(&PaymentMethod::from("paypal"), custom_request("PayPal"))
        .await
        .expect_err("the test mint has no paypal backend");

    assert!(
        !matches!(error, Error::InvalidPaymentMethod),
        "a method differing only in case must pass the scope check, got {error:?}"
    );
}
