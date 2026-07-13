mod htlc_sigall_spending_conditions_tests;
mod htlc_spending_conditions_tests;
mod locktime_spending_conditions_tests;
mod onchain_quote_id_tests;
mod p2pk_sigall_spending_conditions_tests;
mod p2pk_spending_conditions_tests;
mod pending_async_melt_tests;

use cdk_common::nuts::nut17::MAX_CUSTOM_KIND_LEN;
use serde_json::Value;

use super::{validate_custom_quote_fields, PaymentMethod, MAX_REQUEST_FIELD_LEN};
use crate::mint::verification::{validate_custom_payment_method, MAX_CUSTOM_PAYMENT_METHOD_LEN};
use crate::Error;

#[test]
fn custom_quote_request_length_is_bounded() {
    let max_length_request = "a".repeat(MAX_REQUEST_FIELD_LEN);
    validate_custom_quote_fields(&max_length_request, &Value::Null)
        .expect("maximum-length request");

    let oversized_request = "a".repeat(MAX_REQUEST_FIELD_LEN + 1);
    assert!(matches!(
        validate_custom_quote_fields(&oversized_request, &Value::Null),
        Err(Error::RequestFieldTooLarge {
            field,
            actual,
            max,
        }) if field == "request"
            && actual == MAX_REQUEST_FIELD_LEN + 1
            && max == MAX_REQUEST_FIELD_LEN
    ));
}

#[test]
fn custom_quote_extra_length_remains_bounded() {
    let max_length_extra = Value::String("a".repeat(MAX_REQUEST_FIELD_LEN - 2));
    assert_eq!(max_length_extra.to_string().len(), MAX_REQUEST_FIELD_LEN);
    validate_custom_quote_fields("request", &max_length_extra).expect("maximum-length extra");

    let oversized_extra = Value::String("a".repeat(MAX_REQUEST_FIELD_LEN - 1));
    assert!(matches!(
        validate_custom_quote_fields("request", &oversized_extra),
        Err(Error::RequestFieldTooLarge {
            field,
            actual,
            max,
        }) if field == "extra"
            && actual == MAX_REQUEST_FIELD_LEN + 1
            && max == MAX_REQUEST_FIELD_LEN
    ));
}

#[test]
fn custom_quote_method_fits_subscription_kind() {
    let max_length_method = PaymentMethod::from("a".repeat(MAX_CUSTOM_PAYMENT_METHOD_LEN));
    validate_custom_payment_method(&max_length_method).expect("maximum-length method");
    assert_eq!(
        format!("{}_mint_quote", max_length_method.as_str()).len(),
        MAX_CUSTOM_KIND_LEN
    );
    assert_eq!(
        format!("{}_melt_quote", max_length_method.as_str()).len(),
        MAX_CUSTOM_KIND_LEN
    );

    let oversized_method = PaymentMethod::from("a".repeat(MAX_CUSTOM_PAYMENT_METHOD_LEN + 1));
    assert!(matches!(
        validate_custom_payment_method(&oversized_method),
        Err(Error::RequestFieldTooLarge {
            field,
            actual,
            max,
        }) if field == "method"
            && actual == MAX_CUSTOM_PAYMENT_METHOD_LEN + 1
            && max == MAX_CUSTOM_PAYMENT_METHOD_LEN
    ));
}

#[test]
fn custom_quote_method_limit_accounts_for_lowercasing() {
    let method = PaymentMethod::from("İ".repeat(18));
    assert!(matches!(
        validate_custom_payment_method(&method),
        Err(Error::RequestFieldTooLarge {
            field,
            actual: 54,
            max,
        }) if field == "method" && max == MAX_CUSTOM_PAYMENT_METHOD_LEN
    ));
}
