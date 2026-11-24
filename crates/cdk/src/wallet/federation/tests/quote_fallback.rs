//! Quote creation timeouts and fallback to the next member.

use super::*;

#[tokio::test(start_paused = true)]
async fn federated_mint_quote_timeout_falls_back_when_quote_is_not_visible() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_mint_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_mint_quote_response(Ok(expected.clone()));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);

    for connector in [&stalled_member, &healthy_member, &third] {
        connector.push_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        push_mint_quote_status_from_response(connector, &expected);
    }

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
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
    let mint_quote =
        tokio::spawn(async move { connector.post_mint_quote(bolt11_quote_request(3)).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = mint_quote
        .await
        .expect("mint quote task")
        .expect("fallback mint quote");

    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(stalled_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_applies_configured_quote_status_timeout() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    slow_member.set_active_keyset(advertised.clone());
    slow_member.set_mint_quote_status_delay(status_timeout + Duration::from_secs(60));
    slow_member
        .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for id in [member_id(2), member_id(3)] {
        let healthy_member = MockMintConnector::new();
        healthy_member.set_active_keyset(advertised.clone());
        healthy_member
            .set_mint_quote_status_response(Ok(mint_quote_status_response(MintQuoteState::Paid)));
        connectors.insert(
            id,
            Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            quote_status_member_timeout: status_timeout,
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let status = tokio::spawn(async move {
        connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "mint-quote")
            .await
    });

    tokio::task::yield_now().await;
    tokio::time::advance(status_timeout).await;

    let response = status
        .await
        .expect("mint quote status task")
        .expect("mint quote status");
    assert_eq!(response.state(), Some(MintQuoteState::Paid));
}

#[tokio::test]
async fn federated_mint_connector_honors_configured_fanout_concurrency() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let total_calls = Arc::new(AtomicUsize::new(0));
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2), member_id(3)] {
        connectors.insert(
            id,
            Arc::new(PostMintConcurrencyProbeConnector::new(
                advertised.clone(),
                federation_mint_info(&bundle, id),
                share_response(&bundle, id, &outputs),
                Duration::from_millis(20),
                active_calls.clone(),
                max_active_calls.clone(),
                total_calls.clone(),
            )) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
    assert_eq!(max_active_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        total_calls.load(Ordering::SeqCst),
        bundle.public_config.members.len()
    );
    assert_eq!(active_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_creates_melt_quote_with_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(30);

    let unavailable_member = Arc::new(MockMintConnector::new());
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_post_melt_quote_response(Err(crate::Error::Custom(
        "quote coordinator unavailable".to_string(),
    )));
    unavailable_member.set_melt_quote_status_response(Ok(status.clone()));
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_melt_quote_response(Ok(expected.clone()));
    healthy_member.set_melt_quote_status_response(Ok(status.clone()));
    healthy_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

    let unused_member = Arc::new(MockMintConnector::new());
    unused_member.set_active_keyset(advertised);
    unused_member.set_melt_quote_status_response(Ok(status));
    unused_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));

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
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("melt quote");

    assert!(started.elapsed() < fallback_timeout);
    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(unavailable_member.post_melt_quote_call_count(), 1);
    assert_eq!(healthy_member.post_melt_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_melt_quote_pending_submission_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let quote_coordinator = Arc::new(MockMintConnector::new());
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Err(crate::Error::PendingQuote));
    quote_coordinator.set_melt_quote_status_response(Ok(status.clone()));

    let second = Arc::new(MockMintConnector::new());
    second.set_active_keyset(advertised.clone());
    second.set_post_melt_quote_response(Ok(expected.clone()));
    second.set_melt_quote_status_response(Ok(status.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);
    third.set_melt_quote_status_response(Ok(status));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            quote_coordinator.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            second.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(3),
            third.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
    ]);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_fallback_timeout: Duration::from_secs(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let response = connector
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("threshold-visible pending quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(quote_coordinator.post_melt_quote_call_count(), 1);
    assert_eq!(second.post_melt_quote_call_count(), 1);
    assert_eq!(third.post_melt_quote_call_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn federated_melt_quote_timeout_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_melt_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_melt_quote_response(Ok(expected.clone()));
    stalled_member.set_melt_quote_status_response(Ok(status.clone()));

    let second = Arc::new(MockMintConnector::new());
    second.set_active_keyset(advertised.clone());
    second.set_post_melt_quote_response(Ok(expected.clone()));
    second.set_melt_quote_status_response(Ok(status.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);
    third.set_melt_quote_status_response(Ok(status));

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            second.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
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
    let melt_quote =
        tokio::spawn(async move { connector.post_melt_quote(bolt11_melt_quote_request()).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = melt_quote
        .await
        .expect("melt quote task")
        .expect("threshold-visible melt quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(stalled_member.post_melt_quote_call_count(), 1);
    assert_eq!(second.post_melt_quote_call_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn federated_melt_quote_timeout_falls_back_when_quote_is_not_visible() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let fallback_timeout = Duration::from_secs(2);

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_melt_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_melt_quote_response(Ok(expected.clone()));

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_melt_quote_response(Ok(expected.clone()));

    let third = Arc::new(MockMintConnector::new());
    third.set_active_keyset(advertised);

    for connector in [&stalled_member, &healthy_member, &third] {
        connector.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
        connector.push_melt_quote_status_response(Ok(status.clone()));
    }

    let connectors = BTreeMap::from([
        (
            member_id(1),
            stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (
            member_id(2),
            healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
        ),
        (member_id(3), third as Arc<dyn MintConnector + Send + Sync>),
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
    let melt_quote =
        tokio::spawn(async move { connector.post_melt_quote(bolt11_melt_quote_request()).await });

    tokio::task::yield_now().await;
    tokio::time::advance(fallback_timeout).await;

    let response = melt_quote
        .await
        .expect("melt quote task")
        .expect("fallback melt quote");

    assert_melt_quote_matches_expected(&response, &expected);
    assert_eq!(stalled_member.post_melt_quote_call_count(), 1);
    assert_eq!(healthy_member.post_melt_quote_call_count(), 1);
}

#[tokio::test]
async fn federated_mint_connector_requires_threshold_melt_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Ok(expected.clone()));
    quote_coordinator
        .set_melt_quote_status_response(Ok(melt_quote_status_from_create_response(&expected)));
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Err(crate::Error::UnknownQuote));
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
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect_err("melt quote should not be threshold visible");

    assert!(
        err.to_string()
            .contains("insufficient federation melt quote status responses"),
        "{err}"
    );
}

#[tokio::test]
async fn federated_mint_connector_retries_melt_quote_visibility_after_creation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = melt_quote_create_response();
    let status = melt_quote_status_from_create_response(&expected);
    let mut connectors = BTreeMap::new();

    let quote_coordinator = MockMintConnector::new();
    quote_coordinator.set_active_keyset(advertised.clone());
    quote_coordinator.set_post_melt_quote_response(Ok(expected.clone()));
    quote_coordinator.push_melt_quote_status_response(Ok(status.clone()));
    quote_coordinator.push_melt_quote_status_response(Ok(status.clone()));
    connectors.insert(
        member_id(1),
        Arc::new(quote_coordinator) as Arc<dyn MintConnector + Send + Sync>,
    );

    let catching_up_member = MockMintConnector::new();
    catching_up_member.set_active_keyset(advertised.clone());
    catching_up_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
    catching_up_member.push_melt_quote_status_response(Ok(status));
    connectors.insert(
        member_id(2),
        Arc::new(catching_up_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised);
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
    unavailable_member.push_melt_quote_status_response(Err(crate::Error::UnknownQuote));
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
        .post_melt_quote(bolt11_melt_quote_request())
        .await
        .expect("melt quote should become threshold visible");

    assert_melt_quote_matches_expected(&response, &expected);
}

#[tokio::test]
async fn federated_mint_connector_does_not_retry_definitive_quote_errors() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let mut connectors = BTreeMap::new();

    let rejecting_member = MockMintConnector::new();
    rejecting_member.set_active_keyset(advertised.clone());
    rejecting_member.set_post_mint_quote_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(1),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised.clone());
    unused_member.set_post_mint_quote_response(Ok(expected));
    connectors.insert(
        member_id(2),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let final_member = MockMintConnector::new();
    final_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(final_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let err = connector
        .post_mint_quote(bolt11_quote_request(3))
        .await
        .expect_err("definitive error");

    assert!(matches!(err, crate::Error::InvalidPaymentMethod));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_batch_mint_quote_status_ignoring_updated_at() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let statuses = vec![
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-1",
            MintQuoteState::Paid,
        )),
        MintQuoteResponse::Bolt11(mint_quote_status_response_for(
            "mint-quote-2",
            MintQuoteState::Unpaid,
        )),
    ];
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_batch_mint_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, updated_at) in [(member_id(2), 10), (member_id(3), 20)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut member_statuses = statuses.clone();
        for response in &mut member_statuses {
            let MintQuoteResponse::Bolt11(response) = response else {
                panic!("test response must be bolt11");
            };
            response.updated_at = updated_at;
        }
        connector.set_batch_mint_quote_status_response(Ok(member_statuses));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_batch_check_mint_quote_status(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchCheckMintQuoteRequest {
                quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
            },
        )
        .await
        .expect("batch mint quote status");

    assert_eq!(response[0].state(), Some(MintQuoteState::Paid));
    assert_eq!(response[1].state(), Some(MintQuoteState::Unpaid));
}
