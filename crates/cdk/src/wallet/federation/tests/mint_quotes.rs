//! Mint quote creation, coordination and visibility.

use super::*;

#[tokio::test]
async fn federated_mint_connector_uses_threshold_mint_quote_status_ignoring_updated_at() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = mint_quote_status_response(MintQuoteState::Paid);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_mint_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, updated_at) in [(member_id(2), 10), (member_id(3), 20)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut member_status = status.clone();
        member_status.updated_at = updated_at;
        connector.set_mint_quote_status_response(Ok(member_status));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect("mint quote status");

    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_waits_for_matching_mint_quote_status_quorum() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Unpaid)));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector
            .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect("mint quote status");

    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_mint_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
        .await
        .expect_err("one member must not decide mint quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_creates_mint_quote_with_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(30);

    let unavailable_member = Arc::new(MockMintConnector::new());
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_post_mint_quote_response(Err(crate::Error::Custom(
        "quote coordinator unavailable".to_string(),
    )));
    set_mint_quote_status_from_response(&unavailable_member, &expected);
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&healthy_member, &expected);
    healthy_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let unused_member = Arc::new(MockMintConnector::new());
    unused_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&unused_member, &expected);
    unused_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            unavailable_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(3),
            unused_member as Arc<dyn MintConnector + Send + Sync>,
        ),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: fallback_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let started = tokio::time::Instant::now();
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote");

    assert!(started.elapsed() < fallback_timeout);
    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(response.request(), expected.request());
    assert_eq!(response.method(), expected.method());
    assert_eq!(unavailable_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_mint_connector_uses_configured_quote_coordinator_for_quote_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let default_coordinator = MockMintConnector::new();
    default_coordinator.set_active_keyset(advertised.clone());
    default_coordinator.set_post_mint_quote_response(Err(crate::Error::InvalidPaymentMethod));
    set_mint_quote_status_from_response(&default_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(default_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let preferred_member = MockMintConnector::new();
    preferred_member.set_active_keyset(advertised.clone());
    preferred_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&preferred_member, &expected);
    connectors.insert(
        member_id(2),
        Arc::new(preferred_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let third_member = MockMintConnector::new();
    third_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&third_member, &expected);
    connectors.insert(
        member_id(3),
        Arc::new(third_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: Some(member_id(2)),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote");

    QuoteId::from_str(response.quote()).expect("valid assigned mint quote id");
    assert_eq!(connector.quote_coordinator_member_id, member_id(2));
}

#[tokio::test]
async fn federated_mint_connector_requires_threshold_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&quote_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_visibility_attempts: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let err = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect_err("quote should not be threshold visible");

    assert!(
        err.to_string()
            .contains("insufficient federation mint quote status responses"),
        "{err}"
    );
}

#[tokio::test]
async fn federated_mint_connector_retries_mint_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let quote_id = QuoteId::new();
    let expected = bolt11_quote_response(&quote_id, 3);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_mint_quote_response(Ok(expected.clone()));
    push_mint_quote_status_from_response(&quote_coordinator, &expected);
    push_mint_quote_status_from_response(&quote_coordinator, &expected);
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let catching_up_member = MockMintConnector::new();
    catching_up_member.set_active_keyset(advertised.clone());
    catching_up_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    push_mint_quote_status_from_response(&catching_up_member, &expected);
    connectors.insert(
        member_id(2),
        Arc::new(catching_up_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised);
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    unavailable_member.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
    connectors.insert(
        member_id(3),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_visibility_attempts: 2,
            quote_visibility_retry_delay: Duration::from_millis(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect("mint quote should become threshold visible");

    QuoteId::from_str(response.quote()).expect("valid assigned mint quote id");
    assert_eq!(response.request(), expected.request());
}

#[tokio::test]
async fn federated_mint_connector_returns_after_threshold_mint_responses() {
    let bundle = threshold_four_bundle();
    let keyset = active_keyset(&bundle);
    let outputs = vec![blinded_output_for_keyset_amount(
        &keyset,
        1,
        b"threshold mint liveness",
    )];
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(30);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(1), advertised.clone());
    slow_member.set_post_mint_delay(Duration::from_secs(30));
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3), member_id(4)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: request_timeout,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let request = MintRequest {
        quote: "paid-quote".to_string(),
        outputs: outputs.clone(),
        signature: None,
    };
    let mint = tokio::spawn(async move {
        connector
            .post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request)
            .await
    });

    let response = tokio::time::timeout(Duration::from_millis(250), mint)
        .await
        .expect("mint should not wait for slow non-threshold member")
        .expect("mint task")
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &response);
}
