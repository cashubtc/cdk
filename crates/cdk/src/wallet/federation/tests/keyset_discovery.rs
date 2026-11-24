//! Member mint info and keyset discovery, quorums, timeouts and fan-out.

use super::*;

#[tokio::test]
async fn federated_mint_connector_tolerates_offline_member_key_discovery() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let offline_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &offline_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    offline_member.set_mint_keys_error("member unavailable");
    offline_member
        .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(offline_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
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
async fn federated_mint_connector_gets_mint_info_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let expected = federation_mint_info(&bundle, member_id(2)).name("healthy federation member");
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &unavailable_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    unavailable_member.set_mint_info_error("first member unavailable");
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &healthy_member,
        &bundle,
        member_id(2),
        advertised.clone(),
    );
    healthy_member.set_mint_info_response(Ok(expected.clone()));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&unused_member, &bundle, member_id(3), advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector.get_mint_info().await.expect("mint info");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_fetches_lnurl_pay_request_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_lnurl_pay_request_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_lnurl_pay_request_response(Ok(crate::lightning_address::LnurlPayResponse {
        callback: "https://example.com/callback".to_string(),
        min_sendable: 1,
        max_sendable: 1_000,
        metadata: "[]".to_string(),
        tag: Some("payRequest".to_string()),
        reason: None,
    }));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .fetch_lnurl_pay_request("https://example.com/.well-known/lnurlp/alice")
        .await
        .expect("lnurl pay request");

    assert_eq!(response.callback, "https://example.com/callback");
    assert_eq!(response.min_sendable, 1);
    assert_eq!(response.max_sendable, 1_000);
    assert_eq!(response.metadata, "[]");
}

#[tokio::test]
async fn federated_mint_connector_fetches_lnurl_invoice_from_first_healthy_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let unavailable_member = MockMintConnector::new();
    unavailable_member.set_active_keyset(advertised.clone());
    unavailable_member.set_lnurl_invoice_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(unavailable_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let healthy_member = MockMintConnector::new();
    healthy_member.set_active_keyset(advertised.clone());
    healthy_member.set_lnurl_invoice_response(Ok(
        crate::lightning_address::LnurlPayInvoiceResponse {
            pr: Some("bolt11-invoice".to_string()),
            success_action: None,
            routes: None,
            reason: None,
        },
    ));
    connectors.insert(
        member_id(2),
        Arc::new(healthy_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let unused_member = MockMintConnector::new();
    unused_member.set_active_keyset(advertised);
    connectors.insert(
        member_id(3),
        Arc::new(unused_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .fetch_lnurl_invoice("https://example.com/callback?amount=1000")
        .await
        .expect("lnurl invoice");

    assert_eq!(response.pr.as_deref(), Some("bolt11-invoice"));
    assert!(response.success_action.is_none());
    assert!(response.routes.is_none());
    assert!(response.reason.is_none());
}

#[tokio::test]
async fn federated_mint_connector_polls_member_keysets_concurrently() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_mint_keys_barrier(barrier.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(1), connector.get_mint_keys())
            .await
            .expect("member keyset probes should run concurrently")
            .expect("aggregate keysets");

    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_times_out_stalled_mint_info_validation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &stalled_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    stalled_member.set_mint_info_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(1),
        Arc::new(stalled_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
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
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let validation = tokio::spawn(async move { connector.get_mint_keys().await });

    tokio::task::yield_now().await;
    tokio::time::advance(request_timeout).await;

    let response = validation
        .await
        .expect("keyset validation task")
        .expect("healthy threshold after mint info timeout");
    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_times_out_stalled_mint_keys_validation() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &stalled_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    stalled_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(1),
        Arc::new(stalled_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
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
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let validation = tokio::spawn(async move { connector.get_mint_keys().await });

    tokio::task::yield_now().await;
    tokio::time::advance(request_timeout).await;

    let response = validation
        .await
        .expect("keyset validation task")
        .expect("healthy threshold after mint key timeout");
    assert_eq!(response.len(), 1);
    assert_eq!(response[0].id, advertised.id);
}

#[tokio::test]
async fn federated_mint_connector_mints_after_threshold_keyset_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(3), advertised.clone());
    slow_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    slow_member.set_post_mint_response(Ok(share_response(&bundle, member_id(3), &outputs)));
    connectors.insert(
        member_id(3),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should not wait for every keyset advertisement")
    .expect("mint response");

    assert_eq!(response.signatures.len(), outputs.len());
}

#[tokio::test]
async fn federated_mint_connector_reuses_validated_member_keysets() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();
    let mut raw_connectors = Vec::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = Arc::new(MockMintConnector::new());
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.push_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connector.push_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        raw_connectors.push(connector.clone());
        connectors.insert(member_id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("first mint response");

    for connector in raw_connectors {
        connector.set_mint_keys_error("keyset probe should use cache");
    }

    let response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote-2".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("second mint response should reuse validated keysets");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_excludes_bad_keyset_member_from_mint_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut bad_advertised = advertised.clone();
    bad_advertised.input_fee_ppk = bad_advertised.input_fee_ppk.saturating_add(1);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let bad_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&bad_member, &bundle, member_id(3), bad_advertised);
    bad_member.set_post_mint_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    bad_member.set_post_mint_response(Ok(share_response(&bundle, member_id(3), &outputs)));
    connectors.insert(
        member_id(3),
        Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("bad keyset member should not receive mint fanout")
    .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_gets_keyset_after_threshold_member_advertisements() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(3), advertised.clone());
    slow_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(3),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.get_mint_keyset(keyset_id),
    )
    .await
    .expect("keyset fetch should not wait for every member")
    .expect("keyset");

    assert_eq!(response.id, advertised.id);
}

#[tokio::test]
async fn federated_mint_connector_ignores_malformed_keyset_member() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let malformed_member = MockMintConnector::new();
    malformed_member.set_active_keyset(advertised.clone());
    malformed_member
        .set_mint_info_response(Ok(mismatched_federation_mint_info(&bundle, member_id(1))));
    connectors.insert(
        member_id(1),
        Arc::new(malformed_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_mint_keyset(keyset_id)
        .await
        .expect("honest threshold should survive one malformed member");

    assert_eq!(response, advertised);
}

#[tokio::test(start_paused = true)]
async fn federated_mint_connector_bounds_and_times_out_keyset_fanout() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let request_timeout = Duration::from_secs(2);
    let mut connectors = BTreeMap::new();

    let stalled_member = Arc::new(MockMintConnector::new());
    set_member_advertisement_with_keyset(
        &stalled_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    stalled_member.set_mint_keys_barrier(Arc::new(tokio::sync::Barrier::new(2)));
    connectors.insert(
        member_id(1),
        stalled_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let second_member = Arc::new(MockMintConnector::new());
    set_member_advertisement_with_keyset(&second_member, &bundle, member_id(2), advertised.clone());
    connectors.insert(
        member_id(2),
        second_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let third_member = Arc::new(MockMintConnector::new());
    set_member_advertisement_with_keyset(&third_member, &bundle, member_id(3), advertised.clone());
    connectors.insert(
        member_id(3),
        third_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig {
            member_request_timeout: request_timeout,
            fanout_concurrency: 1,
            ..FederatedMintConnectorConfig::default()
        },
    )
    .expect("federated mint connector");
    let lookup = tokio::spawn(async move { connector.get_mint_keyset(keyset_id).await });

    tokio::task::yield_now().await;
    assert_eq!(stalled_member.mint_keyset_call_count(), 1);
    assert_eq!(second_member.mint_keyset_call_count(), 0);
    assert_eq!(third_member.mint_keyset_call_count(), 0);

    tokio::time::advance(request_timeout).await;
    let response = lookup
        .await
        .expect("keyset lookup task")
        .expect("honest threshold after member timeout");

    assert_eq!(response, advertised);
    assert_eq!(second_member.mint_keyset_call_count(), 1);
    assert_eq!(third_member.mint_keyset_call_count(), 1);
}

#[tokio::test]
async fn federated_mint_connector_reports_below_threshold_keyset_quorum() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let honest_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&honest_member, &bundle, member_id(1), advertised.clone());
    connectors.insert(
        member_id(1),
        Arc::new(honest_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_mint_info_response(Ok(mismatched_federation_mint_info(&bundle, member_id)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .get_mint_keyset(keyset_id)
        .await
        .expect_err("one valid member cannot satisfy a two-member threshold");
    let detail = error.to_string();

    assert!(detail.contains("required 2, received 1"), "{detail}");
    assert!(detail.contains("member 2"), "{detail}");
    assert!(detail.contains("member 3"), "{detail}");
}

#[tokio::test]
async fn federated_mint_connector_rejects_conflicting_keyset_advertisements_deterministically() {
    let bundle = threshold_bundle();
    let keyset_id = bundle.public_config.keysets[0].id;
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        let mut conflicting = advertised.clone();
        conflicting.input_fee_ppk = u64::from(member_id.get());
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, conflicting);
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let first = connector
        .get_mint_keyset(keyset_id)
        .await
        .expect_err("conflicting member advertisements must fail")
        .to_string();
    let second = connector
        .get_mint_keyset(keyset_id)
        .await
        .expect_err("repeated conflict must fail identically")
        .to_string();

    assert_eq!(first, second);
    assert!(first.contains("required 2, received 0"), "{first}");
}

#[tokio::test]
async fn federated_mint_connector_fans_out_mint_members_concurrently() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_mint_barrier(barrier.clone());
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&failing_member, &bundle, member_id(3), advertised);
    failing_member
        .set_post_mint_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(3),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        ),
    )
    .await
    .expect("mint member requests should run concurrently")
    .expect("mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn federated_mint_connector_aggregates_batch_mint_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec!["paid-quote-1".to_string(), "paid-quote-2".to_string()],
                quote_amounts: None,
                outputs: outputs.clone(),
                signatures: None,
            },
        )
        .await
        .expect("batch mint response");

    assert_aggregate_response_valid(&bundle, &outputs, &response);
}
