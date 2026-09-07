use super::*;

#[test]
fn runtime_config_builds_mint_quote_consensus_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let envelope = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(10),
        bolt11_quote_response(&quote, 10),
    );

    assert_eq!(envelope.federation_id, runtime_config.federation_id());
    let quote_string = quote.to_string();
    assert_eq!(envelope.operation.quote(), Some(quote_string.as_str()));
    assert!(matches!(
        envelope.operation,
        FederationOperation::MintQuote(_)
    ));

    let mut journal = InMemoryFederationJournal::new();
    let first_response = journal.append(envelope.clone()).expect("append quote");
    let duplicate_response = journal.append(envelope).expect("append duplicate quote");
    let conflict = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(11),
        bolt11_quote_response(&quote, 11),
    );

    assert_eq!(
        first_response.entry().operation_id,
        duplicate_response.entry().operation_id
    );
    assert!(matches!(
        journal.append(conflict),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[test]
fn runtime_config_admission_validates_quote_and_payment_observation_submissions() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let mint_quote = FederationMintQuoteOperation {
        quote: quote.to_string(),
        request_lookup_id: PaymentIdentifier::QuoteId(quote.clone()),
        request: bolt11_quote_request(10),
        response: bolt11_quote_response(&quote, 10),
    };
    runtime_config
        .validate_federated_mint_quote_submission(&mint_quote)
        .expect("valid mint quote submission");

    let mut mismatched_response_quote = mint_quote.clone();
    mismatched_response_quote.response = bolt11_quote_response(&QuoteId::new(), 10);
    assert!(matches!(
        runtime_config.validate_federated_mint_quote_submission(&mismatched_response_quote),
        Err(Error::UnknownQuote)
    ));

    let mut empty_payment_request = mint_quote.clone();
    if let MintQuoteResponse::Bolt11(response) = &mut empty_payment_request.response {
        response.request.clear();
    }
    assert!(runtime_config
        .validate_federated_mint_quote_submission(&empty_payment_request)
        .is_err());

    let mint_observation = FederationMintQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        state: MintQuoteState::Paid,
        amount_paid: Amount::from(10),
        amount_issued: Amount::ZERO,
        payment_id: None,
        payment_proof: Some("settled".to_string()),
    };
    runtime_config
        .validate_federated_mint_payment_observation_submission(&mint_observation)
        .expect("valid mint payment observation");

    let mut unknown_mint_observer = mint_observation.clone();
    unknown_mint_observer.observer = MemberId::new(99).expect("unknown member");
    assert!(matches!(
        runtime_config
            .validate_federated_mint_payment_observation_submission(&unknown_mint_observer),
        Err(Error::Federation(FederationError::UnknownMember { .. }))
    ));

    let mut invalid_mint_method = mint_observation.clone();
    invalid_mint_method.method = PaymentMethod::Custom("bad/method".to_string());
    assert!(matches!(
        runtime_config.validate_federated_mint_payment_observation_submission(&invalid_mint_method),
        Err(Error::InvalidPaymentMethod)
    ));

    let mut over_issued_mint = mint_observation.clone();
    over_issued_mint.amount_issued = Amount::from(11);
    assert!(runtime_config
        .validate_federated_mint_payment_observation_submission(&over_issued_mint)
        .is_err());

    let request = pending_melt_quote_request(1_000);
    let melt_quote = FederationMeltQuoteOperation {
        quote: quote.to_string(),
        response: test_melt_quote_response(&quote, &request).into(),
        metadata: FederationMeltQuoteMetadata::default(),
        request,
    };
    runtime_config
        .validate_federated_melt_quote_submission(&melt_quote)
        .expect("valid melt quote submission");

    let request = MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
        method: "custom".to_string(),
        request: String::new(),
        unit: CurrencyUnit::Sat,
        amount: None,
        extra: serde_json::Value::Null,
    });
    let empty_melt_quote = FederationMeltQuoteOperation {
        quote: quote.to_string(),
        response: test_melt_quote_response(&quote, &request).into(),
        metadata: FederationMeltQuoteMetadata::default(),
        request,
    };
    assert!(runtime_config
        .validate_federated_melt_quote_submission(&empty_melt_quote)
        .is_err());

    let melt_observation = FederationMeltQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
        state: MeltQuoteState::Paid,
        amount: Amount::from(9),
        fee_reserve: Amount::from(1),
        total_spent: Some(Amount::from(10)),
        payment_id: None,
        payment_proof: Some("preimage".to_string()),
    };
    runtime_config
        .validate_federated_melt_payment_observation_submission(&melt_observation)
        .expect("valid melt payment observation");

    let mut invalid_melt_method = melt_observation.clone();
    invalid_melt_method.method = PaymentMethod::Custom("bad/method".to_string());
    assert!(matches!(
        runtime_config.validate_federated_melt_payment_observation_submission(&invalid_melt_method),
        Err(Error::InvalidPaymentMethod)
    ));
}

#[test]
fn mint_quote_conversion_preserves_bolt11_accounting() {
    let quote = QuoteId::new();
    let mut response = bolt11_quote_response(&quote, 10);
    let MintQuoteResponse::Bolt11(response) = &mut response else {
        panic!("expected bolt11 response");
    };
    response.amount_paid = Amount::from(7);
    response.amount_issued = Amount::from(3);
    response.state = MintQuoteState::Paid;
    response.updated_at = 1_700_000_123;

    let operation = FederationMintQuoteOperation {
        quote: quote.to_string(),
        request_lookup_id: PaymentIdentifier::QuoteId(quote),
        request: bolt11_quote_request(10),
        response: MintQuoteResponse::Bolt11(response.clone()),
    };
    let converted = super::super::helpers::mint_quote_from_federation_operation(&operation)
        .expect("convert mint quote");

    assert_eq!(
        converted.amount_paid(),
        Amount::from(7).with_unit(CurrencyUnit::Sat)
    );
    assert_eq!(
        converted.amount_issued(),
        Amount::from(3).with_unit(CurrencyUnit::Sat)
    );
    assert_eq!(converted.created_time, response.updated_at);
}

#[tokio::test]
async fn apply_federation_mint_quote_uses_committed_response_time() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let mut response = bolt11_quote_response(&quote, 10);
    let MintQuoteResponse::Bolt11(response) = &mut response else {
        panic!("expected bolt11 response");
    };
    response.updated_at = 1_700_000_456;
    let expected_created_time = response.updated_at;
    let entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(10),
            MintQuoteResponse::Bolt11(response.clone()),
        ),
    );

    persist_and_apply_entry(&mint, &entry).await;

    let stored = localstore
        .get_mint_quote(&quote)
        .await
        .expect("mint quote lookup")
        .expect("mint quote");
    assert_eq!(stored.created_time, expected_created_time);
}

#[tokio::test]
async fn federation_mint_lifecycle_publishes_quote_events_after_commit() {
    let runtime_config = runtime_config();
    let (mint, _) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let mut subscription = mint
        .pubsub_manager()
        .subscribe(cdk_common::subscription::Params {
            kind: cdk_common::nut17::Kind::Bolt11MintQuote,
            filters: vec![quote.to_string()],
            id: Arc::new(cdk_common::subscription::SubId::from(
                "federation-mint-lifecycle",
            )),
        })
        .expect("subscribe to mint quote");

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let created_event =
        tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
            .await
            .expect("created quote event timeout")
            .expect("created quote event");
    match created_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Unpaid);
        }
        payload => panic!("unexpected created quote payload: {payload:?}"),
    }

    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("federation-mint-lifecycle-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let paid_event = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
        .await
        .expect("paid quote event timeout")
        .expect("paid quote event");
    match paid_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Paid);
        }
        payload => panic!("unexpected paid quote payload: {payload:?}"),
    }

    let output = output_for_runtime(&runtime_config, b"federation mint lifecycle output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let issued_event = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
        .await
        .expect("issued quote event timeout")
        .expect("issued quote event");
    match issued_event.into_inner() {
        cdk_common::NotificationPayload::MintQuoteBolt11Response(response) => {
            assert_eq!(response.quote, quote);
            assert_eq!(response.state, MintQuoteState::Issued);
        }
        payload => panic!("unexpected issued quote payload: {payload:?}"),
    }
}
