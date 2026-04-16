#[test]
fn test_payment_identifier_unsupported_kind() {
    let result = PaymentIdentifier::new("unsupported_kind", "123");
    assert!(matches!(result, Err(Error::UnsupportedPaymentOption)));
}
