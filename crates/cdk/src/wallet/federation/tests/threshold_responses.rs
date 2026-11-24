//! Threshold response collection: retries, catch-up waits, rejections and ambiguity.

use super::*;

#[tokio::test]
async fn federated_mint_connector_returns_after_three_of_five_with_two_slow_members() {
    let bundle = threshold_five_bundle();
    let keyset = active_keyset(&bundle);
    let outputs = vec![blinded_output_for_keyset_amount(
        &keyset,
        1,
        b"three of five mint liveness",
    )];
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member, &outputs)));
        connectors.insert(
            member,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }
    for member in [member_id(4), member_id(5)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member, advertised.clone());
        connector.set_post_mint_delay(Duration::from_secs(30));
        connector.set_post_mint_response(Ok(share_response(&bundle, member, &outputs)));
        connectors.insert(
            member,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::from_secs(30),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let request = MintRequest {
        quote: "paid-three-of-five-quote".to_string(),
        outputs: outputs.clone(),
        signature: None,
    };
    let response = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_mint(&PaymentMethod::Known(KnownMethod::Bolt11), request),
    )
    .await
    .expect("three valid shares must cancel two slow member requests")
    .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_retries_retryable_member_request_failure() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let flaky_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&flaky_member, &bundle, member_id(1), advertised.clone());
    flaky_member.push_post_mint_response(Err(crate::Error::Custom(
        "temporary member transport failure".to_string(),
    )));
    flaky_member.push_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(flaky_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
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
}

#[tokio::test]
async fn federated_mint_connector_fans_signing_request_out_to_every_member() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let mut observed_connectors = Vec::new();

    for id in [
        member_id(1),
        member_id(2),
        member_id(3),
        member_id(4),
        member_id(5),
    ] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        if id == member_id(1) {
            connector.set_post_swap_response(Err(crate::Error::Timeout));
        } else {
            connector.set_post_swap_response(Ok(SwapResponse {
                signatures: share_response(&bundle, id, &outputs).signatures,
            }));
        }
        observed_connectors.push(connector.clone());
        connectors.insert(id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let response = connector
        .post_swap(request.clone())
        .await
        .expect("three compatible shares from the full roster");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
    for member in observed_connectors {
        assert_eq!(
            member.captured_swap_requests(),
            vec![request.clone()],
            "every configured federation member must receive the signing request"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_waits_for_catching_up_member_with_identical_request() {
    let bundle = threshold_four_bundle();
    let outputs = vec![blinded_output_for_keyset_amount(
        &active_keyset(&bundle),
        1,
        b"catch-up retry output",
    )];
    let advertised = advertised_keyset(&bundle);
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let catching_up_member = Arc::new(MockMintConnector::new());
    set_member_advertisement_with_keyset(
        &catching_up_member,
        &bundle,
        member_id(3),
        advertised.clone(),
    );
    catching_up_member.push_post_swap_response(Err(federation_catch_up_error(member_id(3))));
    catching_up_member.push_post_swap_response(Err(federation_catch_up_error(member_id(3))));
    catching_up_member.push_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(3), &outputs).signatures,
    }));
    connectors.insert(
        member_id(3),
        catching_up_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(4), advertised);
    rejecting_member.set_post_swap_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(4),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_swap(request.clone())
        .await
        .expect("catching-up member should become available before the retry deadline");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
    assert_eq!(
        catching_up_member.captured_swap_requests(),
        vec![request.clone(), request.clone(), request],
        "catch-up retries must reuse the exact deterministic request"
    );
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_returns_typed_ambiguous_error_after_catch_up_deadline() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let request = SwapRequest::new(Vec::new(), outputs.clone());
    let mut connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let mut catching_up_members = Vec::new();
    for id in [member_id(3), member_id(4), member_id(5)] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        for _ in 0..=FEDERATION_CATCH_UP_MAX_RETRIES {
            connector.push_post_swap_response(Err(federation_catch_up_error(id)));
        }
        catching_up_members.push(connector.clone());
        connectors.insert(id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .post_swap(request.clone())
        .await
        .expect_err("two shares cannot satisfy a three-share threshold");

    assert!(matches!(error, crate::Error::FederationCatchUp { .. }));
    assert!(!error.is_definitive_failure());
    for member in catching_up_members {
        let requests = member.captured_swap_requests();
        assert_eq!(
            requests.len(),
            usize::from(FEDERATION_CATCH_UP_MAX_RETRIES) + 1
        );
        assert!(requests.iter().all(|candidate| candidate == &request));
    }
}

#[tokio::test]
async fn federated_mint_connector_retries_rate_limited_member_request() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let rate_limited_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &rate_limited_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    rate_limited_member.push_post_mint_response(Err(crate::Error::HttpError(
        Some(429),
        "too many requests".to_string(),
    )));
    rate_limited_member.push_post_mint_response(Ok(share_response(
        &bundle,
        member_id(1),
        &outputs,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(rate_limited_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
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
}

#[tokio::test]
async fn federated_mint_connector_tolerates_mixed_final_pending_and_rejected_value_responses() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let final_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&final_member, &bundle, member_id(1), advertised.clone());
    final_member.set_post_mint_response(Ok(share_response(&bundle, member_id(1), &outputs)));
    final_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(1), &outputs).signatures,
    }));
    final_member.set_post_melt_response(Ok(melt_response(&bundle, member_id(1), &change_outputs)));
    connectors.insert(
        member_id(1),
        Arc::new(final_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let pending_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &pending_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    pending_member.push_post_mint_response(Err(pending_finalization_error(1)));
    pending_member.push_post_mint_response(Ok(share_response(&bundle, member_id(2), &outputs)));
    pending_member.push_post_swap_response(Err(pending_finalization_error(2)));
    pending_member.push_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(2), &outputs).signatures,
    }));
    pending_member.push_post_melt_response(Err(pending_finalization_error(3)));
    pending_member.push_post_melt_response(Ok(melt_response(
        &bundle,
        member_id(2),
        &change_outputs,
    )));
    connectors.insert(
        member_id(2),
        Arc::new(pending_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let rejecting_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&rejecting_member, &bundle, member_id(3), advertised);
    rejecting_member.set_post_mint_response(Err(crate::Error::InvalidPaymentMethod));
    rejecting_member.set_post_swap_response(Err(crate::Error::InvalidPaymentMethod));
    rejecting_member.set_post_melt_response(Err(crate::Error::InvalidPaymentMethod));
    connectors.insert(
        member_id(3),
        Arc::new(rejecting_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let mint_response = connector
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
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_returns_threshold_rejection_without_waiting_for_slow_members() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let rejection = cdk_common::error::ErrorResponse::from(crate::Error::TokenAlreadySpent);

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::MintErrorResponse(rejection.clone())));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let duplicate_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &duplicate_member,
        &bundle,
        member_id(4),
        advertised.clone(),
    );
    duplicate_member.set_post_swap_delay(Duration::from_secs(30));
    duplicate_member.set_post_swap_response(Err(crate::Error::DuplicateInputs));
    connectors.insert(
        member_id(4),
        Arc::new(duplicate_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unavailable_member, &bundle, member_id(5), advertised);
    unavailable_member.set_post_swap_delay(Duration::from_secs(30));
    unavailable_member.set_post_swap_response(Err(crate::Error::Timeout));
    connectors.insert(
        member_id(5),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_swap(SwapRequest::new(Vec::new(), outputs)),
    )
    .await
    .expect("matching threshold rejection should cancel slow member requests");

    assert!(matches!(result, Err(crate::Error::TokenAlreadySpent)));
}

#[tokio::test]
async fn federated_mint_connector_keeps_disagreeing_rejections_ambiguous() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for (member_id, error) in [
        (member_id(1), crate::Error::TokenAlreadySpent),
        (member_id(2), crate::Error::TokenAlreadySpent),
        (member_id(3), crate::Error::DuplicateInputs),
        (member_id(4), crate::Error::DuplicateInputs),
        (member_id(5), crate::Error::Timeout),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(error));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_requires_exact_error_response_agreement() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let first_rejection = cdk_common::error::ErrorResponse::new(
        cdk_common::error::ErrorCode::TokenAlreadySpent,
        "spent in operation one".to_string(),
    );
    let second_rejection = cdk_common::error::ErrorResponse::new(
        cdk_common::error::ErrorCode::TokenAlreadySpent,
        "spent in operation two".to_string(),
    );
    let mut connectors = BTreeMap::new();

    for (id, rejection) in [
        (member_id(1), first_rejection.clone()),
        (member_id(2), first_rejection),
        (member_id(3), second_rejection.clone()),
        (member_id(4), second_rejection),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::MintErrorResponse(rejection)));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unavailable_member, &bundle, member_id(5), advertised);
    unavailable_member.set_post_swap_response(Err(crate::Error::Timeout));
    connectors.insert(
        member_id(5),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_round_trips_mint_and_melt_cashu_rejections() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let slow_member_id = member_id(3);

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        if member_id == slow_member_id {
            connector.set_post_mint_delay(Duration::from_secs(30));
            connector.set_post_melt_delay(Duration::from_secs(30));
        }
        connector.set_post_mint_response(Err(crate::Error::IssuedQuote));
        connector.set_post_melt_response(Err(crate::Error::TokenAlreadySpent));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let mint_result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "issued-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("matching mint rejection should cancel slow member requests");
    assert!(matches!(mint_result, Err(crate::Error::IssuedQuote)));

    let melt_result = tokio::time::timeout(
        Duration::from_millis(250),
        connector.post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new("melt-quote".to_string(), Vec::new(), None),
        ),
    )
    .await
    .expect("matching melt rejection should cancel slow member requests");
    assert!(matches!(melt_result, Err(crate::Error::TokenAlreadySpent)));
}

#[tokio::test]
async fn federated_mint_connector_keeps_timeouts_and_invalid_shares_ambiguous() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut timeout_connectors = BTreeMap::new();
    let mut invalid_share_connectors = BTreeMap::new();

    for id in [member_id(1), member_id(2), member_id(3)] {
        let timeout_connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&timeout_connector, &bundle, id, advertised.clone());
        timeout_connector.set_post_swap_delay(Duration::from_millis(50));
        timeout_connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, id, &outputs).signatures,
        }));
        timeout_connectors.insert(
            id,
            Arc::new(timeout_connector) as Arc<dyn MintConnector + Send + Sync>,
        );

        let invalid_connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&invalid_connector, &bundle, id, advertised.clone());
        let wrong_signer = if id == member_id(3) {
            member_id(1)
        } else {
            member_id(3)
        };
        invalid_connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, wrong_signer, &outputs).signatures,
        }));
        invalid_share_connectors.insert(
            id,
            Arc::new(invalid_connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let timeout_connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        timeout_connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::from_millis(1),
            member_request_max_retries: 0,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("timeout connector");
    let timeout_result = timeout_connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await;
    assert!(matches!(
        timeout_result,
        Err(crate::Error::InvalidMintResponse(_))
    ));

    let invalid_share_connector =
        FederatedMintConnector::new(bundle.public_config.clone(), invalid_share_connectors)
            .expect("invalid-share connector");
    let invalid_share_result = invalid_share_connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;
    assert!(matches!(
        invalid_share_result,
        Err(crate::Error::InvalidMintResponse(_))
    ));
}

#[tokio::test]
async fn federated_mint_connector_tolerates_slow_offline_and_bad_share_members() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let slow_delay = Duration::from_millis(10);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, member_id, &outputs).signatures,
        }));
        connector.set_post_melt_response(Ok(melt_response(&bundle, member_id, &change_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &offline_member,
        &bundle,
        member_id(3),
        advertised.clone(),
    );
    offline_member.set_post_mint_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_swap_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_melt_response(Err(crate::Error::Custom("member offline".to_string())));
    connectors.insert(
        member_id(3),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let bad_share_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &bad_share_member,
        &bundle,
        member_id(4),
        advertised.clone(),
    );
    bad_share_member.set_post_mint_response(Ok(share_response(&bundle, member_id(5), &outputs)));
    bad_share_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(5), &outputs).signatures,
    }));
    bad_share_member.set_post_melt_response(Ok(melt_response(
        &bundle,
        member_id(5),
        &change_outputs,
    )));
    connectors.insert(
        member_id(4),
        Arc::new(bad_share_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(5), advertised);
    slow_member.set_post_mint_delay(slow_delay);
    slow_member.set_post_swap_delay(slow_delay);
    slow_member.set_post_melt_delay(slow_delay);
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(5), &outputs)));
    slow_member.set_post_swap_response(Ok(SwapResponse {
        signatures: share_response(&bundle, member_id(5), &outputs).signatures,
    }));
    slow_member.set_post_melt_response(Ok(melt_response(&bundle, member_id(5), &change_outputs)));
    connectors.insert(
        member_id(5),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::from_secs(1),
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");

    let mint_response = connector
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
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[test]
fn federated_share_batch_rejects_metadata_before_parsing_signature_points() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
        FederatedMintConnectorConfig::default(),
    )
    .expect("federated mint connector");
    let mut response = share_response(&bundle, member_id(1), &outputs);
    response.signatures[0].amount = Amount::from(2);
    response.signatures[0].c = identity_secret(member_id(1)).public_key();

    let error = connector
        .valid_member_share_set(
            &outputs,
            member_id(1),
            &response,
            super::super::ShareAmountValidation::Exact,
        )
        .expect_err("mismatched signature metadata");

    assert!(error.to_string().contains("signature amount"));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_live_members_for_wallet_operations() {
    let bundle = threshold_five_bundle();
    let outputs = blinded_outputs(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let check_ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"threshold-live-check-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"threshold-live-check-y-2")),
    ];
    let check_response = proof_state_response(&check_ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3), member_id(5)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.set_post_swap_response(Ok(SwapResponse {
            signatures: share_response(&bundle, member_id, &outputs).signatures,
        }));
        connector.set_post_melt_response(Ok(melt_response(&bundle, member_id, &change_outputs)));
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &restore_outputs)));
        connector.set_check_state_response(Ok(check_response.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&offline_member, &bundle, member_id(4), advertised);
    offline_member.set_mint_keys_error("member offline");
    offline_member.set_post_mint_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_swap_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member.set_post_melt_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member._set_restore_response(Err(crate::Error::Custom("member offline".to_string())));
    offline_member
        .set_check_state_response(Err(crate::Error::Custom("member offline".to_string())));
    connectors.insert(
        member_id(4),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");

    let mint_response = connector
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
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let change = melt_response.change().cloned().expect("change signatures");
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore response");
    assert_eq!(restore_response.outputs, restore_outputs);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&restore_outputs),
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );

    let state_response = connector
        .post_check_state(CheckStateRequest { ys: check_ys })
        .await
        .expect("check-state response");
    assert_eq!(state_response, check_response);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_quote_timeout_falls_back_to_next_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let stalled_quote = bolt11_quote_response(&QuoteId::new(), 3);
    let expected = bolt11_quote_response(&QuoteId::new(), 3);
    let fallback_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = Arc::new(MockMintConnector::new());
    stalled_member.set_active_keyset(advertised.clone());
    stalled_member.set_post_mint_quote_delay(fallback_timeout + Duration::from_secs(60));
    stalled_member.set_post_mint_quote_response(Ok(stalled_quote));
    set_mint_quote_status_from_response(&stalled_member, &expected);
    connectors.insert(
        member_id(1),
        stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = Arc::new(MockMintConnector::new());
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_post_mint_quote_response(Ok(expected.clone()));
    set_mint_quote_status_from_response(&healthy_member, &expected);
    connectors.insert(
        member_id(2),
        healthy_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    set_mint_quote_status_from_response(&unused_member, &expected);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

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
        .expect("mint quote");

    QuoteId::from_str(response.quote()).expect("valid mint quote id");
    assert_eq!(response.request(), expected.request());
    assert_eq!(response.method(), expected.method());
    assert_eq!(stalled_member.post_mint_quote_call_count(), 1);
    assert_eq!(healthy_member.post_mint_quote_call_count(), 1);
}
