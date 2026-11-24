//! Auth wallets, output and mint-info checks before fan-out, and share reuse.

use super::*;

#[tokio::test]
async fn federated_mint_connector_reads_auth_wallet_from_first_available_member() {
    let bundle = threshold_bundle();
    let auth_wallet = test_auth_wallet().await;
    let mut connectors = BTreeMap::new();

    let first_member = Arc::new(MockMintConnector::new());
    connectors.insert(
        member_id(1),
        first_member.clone() as Arc<dyn MintConnector + Send + Sync>,
    );

    let fallback = Arc::new(MockMintConnector::new());
    fallback.set_stored_auth_wallet(Some(auth_wallet.clone()));
    connectors.insert(
        member_id(2),
        fallback as Arc<dyn MintConnector + Send + Sync>,
    );

    let final_member = Arc::new(MockMintConnector::new());
    connectors.insert(
        member_id(3),
        final_member as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let stored = connector.get_auth_wallet().await.expect("auth wallet");

    assert_eq!(stored.mint_url, auth_wallet.mint_url);
    assert!(first_member.get_auth_wallet().await.is_none());
}

#[tokio::test]
async fn federated_mint_connector_rejects_non_v3_melt_change_outputs() {
    let bundle = threshold_bundle();
    let change_outputs = blinded_change_outputs(&bundle);
    let mut bad_change_outputs = change_outputs.clone();
    let keyset = &bundle.public_config.keysets[0];
    bad_change_outputs[0].keyset_id = Id::v2_from_data(
        &keyset.keys(),
        &keyset.unit,
        keyset.input_fee_ppk,
        keyset.final_expiry,
    );
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors_with_melt_change(&bundle, &change_outputs),
    )
    .expect("federated mint connector");
    let result = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                "melt-quote".to_string(),
                Vec::new(),
                Some(bad_change_outputs),
            ),
        )
        .await;

    assert!(matches!(
        result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_non_v3_mint_swap_and_restore_outputs_before_fanout() {
    let bundle = threshold_bundle();
    let connector = || {
        FederatedMintConnector::new(
            bundle.public_config.clone(),
            member_connectors_with_only_keysets(&bundle),
        )
        .expect("federated mint connector")
    };

    let mint_result = connector()
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "mint-quote".to_string(),
                outputs: non_v3_blinded_outputs(&bundle, b"non-v3 mint output"),
                signature: None,
            },
        )
        .await;
    assert!(matches!(
        mint_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let batch_mint_result = connector()
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec!["batch-mint-quote".to_string()],
                quote_amounts: None,
                outputs: non_v3_blinded_outputs(&bundle, b"non-v3 batch mint output"),
                signatures: None,
            },
        )
        .await;
    assert!(matches!(
        batch_mint_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let swap_result = connector()
        .post_swap(SwapRequest::new(
            Vec::new(),
            non_v3_blinded_outputs(&bundle, b"non-v3 swap output"),
        ))
        .await;
    assert!(matches!(
        swap_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    let restore_result = connector()
        .post_restore(RestoreRequest {
            outputs: non_v3_blinded_outputs(&bundle, b"non-v3 restore output"),
        })
        .await;
    assert!(matches!(
        restore_result,
        Err(crate::Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_missing_federation_mint_info_before_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let advertised = advertised_keyset(&bundle);

    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
                && message.contains("did not advertise federation mint info")
    ));
}

#[tokio::test]
async fn federated_mint_connector_rejects_mismatched_federation_mint_info_before_fanout() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let advertised = advertised_keyset(&bundle);

    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        bad_member.set_active_keyset(advertised.clone());
        bad_member.set_mint_info_response(Ok(mismatched_federation_mint_info(&bundle, member_id)));
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
                && message.contains("Federation id mismatch")
    ));
}

#[tokio::test]
async fn federated_mint_connector_fails_without_threshold_v3_member_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let mut advertised = advertised_keyset(&bundle);
    let expected = &bundle.public_config.keysets[0];
    advertised.id = Id::v2_from_data(
        &advertised.keys,
        &expected.unit,
        expected.input_fee_ppk,
        expected.final_expiry,
    );
    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        set_member_advertisement_with_keyset(&bad_member, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
    ));
}

#[tokio::test]
async fn federated_mint_connector_fails_without_threshold_matching_member_advertisements() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let mut connectors = member_connectors(&bundle, &outputs);
    let mut advertised = advertised_keyset(&bundle);
    advertised.keys = cdk_common::Keys::new(BTreeMap::new());
    for member_id in [member_id(1), member_id(2)] {
        let bad_member = MockMintConnector::new();
        set_member_advertisement_with_keyset(&bad_member, &bundle, member_id, advertised.clone());
        connectors.insert(
            member_id,
            Arc::new(bad_member) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: "paid-quote".to_string(),
                outputs,
                signature: None,
            },
        ),
    )
    .await
    .expect("mint should fail before signing fanout");

    assert!(matches!(
        result,
        Err(crate::Error::InvalidMintResponse(message))
            if message.contains("insufficient federation keyset advertisements")
    ));
}

#[tokio::test]
async fn federated_melt_accepts_partial_change_and_recovers_the_same_signatures() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let outputs = blinded_change_outputs(&bundle);
    assert!(outputs.len() > 1);
    let signed_outputs = &outputs[..1];
    let mut connectors = BTreeMap::new();
    for id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        let response = melt_response(&bundle, id, signed_outputs);
        connector.set_post_melt_response(Ok(response.clone()));
        let MeltQuoteResponse::Bolt11(status) = response else {
            panic!("expected bolt11 response");
        };
        connector.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }
    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federation connector");
    let immediate = connector
        .post_melt(
            &PaymentMethod::BOLT11,
            MeltRequest::new("melt-quote".to_string(), Vec::new(), Some(outputs.clone())),
        )
        .await
        .expect("unused blank change outputs are allowed");
    let recovered = connector
        .get_melt_quote_status(PaymentMethod::BOLT11, "melt-quote")
        .await
        .expect("status recovery verifies the same output prefix");
    assert_eq!(immediate, recovered);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(signed_outputs),
        &MintResponse {
            signatures: recovered.change().cloned().expect("verified change"),
        },
    );

    let short = share_response(
        &bundle,
        member_id(1),
        &signed_change_outputs(signed_outputs),
    );
    assert!(connector
        .valid_member_share_set(&outputs, member_id(1), &short, ShareAmountValidation::Exact)
        .is_err());
    assert!(connector
        .valid_member_share_set(
            &outputs,
            member_id(1),
            &short,
            ShareAmountValidation::Restore
        )
        .is_err());
    let excess = share_response(&bundle, member_id(1), &signed_change_outputs(&outputs));
    assert!(connector
        .valid_member_share_set(
            signed_outputs,
            member_id(1),
            &excess,
            ShareAmountValidation::Change
        )
        .is_err());
}

#[test]
fn request_share_reuse_is_bound_to_outputs_members_and_responses() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new_with_config(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
        FederatedMintConnectorConfig::default(),
    )
    .expect("connector");
    let responses = [member_id(1), member_id(2)]
        .into_iter()
        .map(|id| (id, share_response(&bundle, id, &outputs)))
        .collect::<Vec<_>>();
    let mut uncached_work = 0;
    let mut retained = super::super::RequestShareVerification::new(
        &connector,
        super::super::ShareAmountValidation::Exact,
    );
    // Reproduce the old collector/collector-final/caller verification passes and
    // compare the same request using retained, input-bound verified shares.
    for _ in 0..3 {
        let mut fresh = super::super::RequestShareVerification::new(
            &connector,
            super::super::ShareAmountValidation::Exact,
        );
        let expected = fresh
            .aggregate(&outputs, &responses)
            .expect("fresh aggregate");
        uncached_work += fresh.shares_verified;
        let actual = retained
            .aggregate(&outputs, &responses)
            .expect("retained aggregate");
        assert_eq!(actual, expected);
        assert_aggregate_response_valid(&bundle, &outputs, &actual);
    }
    assert_eq!(uncached_work, retained.shares_verified * 3);
    eprintln!("Wallet repeated-pass comparison: old_shares_verified={uncached_work}, retained_shares_verified={}", retained.shares_verified);

    let other_outputs = blinded_outputs_for_keyset(&active_keyset(&bundle), b"different request");
    assert!(retained.aggregate(&other_outputs, &responses).is_err());
    assert!(retained.aggregate(&outputs, &responses).is_ok());
    let mut changed = responses.clone();
    changed[0].1 = share_response(&bundle, member_id(3), &outputs);
    assert!(
        retained.aggregate(&outputs, &changed).is_err(),
        "wrong member share must not reuse the old verification"
    );
    changed[0].1 = responses[0].1.clone();
    changed[0].1.signatures[0].amount = Amount::from(2);
    assert!(
        retained.aggregate(&outputs, &changed).is_err(),
        "changed metadata must be revalidated"
    );
}
