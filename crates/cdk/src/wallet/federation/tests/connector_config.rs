//! Connector construction: aggregate keysets, invite codes, member policy and URLs.

use super::*;

#[test]
fn wallet_connector_accepts_matching_v3_aggregate_keysets() {
    let bundle = bundle();
    let advertised = advertised_keyset(&bundle);

    bundle
        .public_config
        .validate_member_advertised_keysets(member_id(1), &[advertised])
        .expect("matching member keysets");
}

#[test]
fn wallet_connector_rejects_non_v3_aggregate_keysets() {
    let bundle = bundle();
    let mut advertised = advertised_keyset(&bundle);
    let expected = &bundle.public_config.keysets[0];
    advertised.id = Id::v2_from_data(
        &advertised.keys,
        &expected.unit,
        expected.input_fee_ppk,
        expected.final_expiry,
    );

    assert!(matches!(
        bundle
            .public_config
            .validate_member_advertised_keysets(member_id(1), &[advertised],),
        Err(FederationError::UnknownFederatedKeyset { .. }
            | FederationError::InvalidFederatedKeysetVersion { .. })
    ));
}

#[test]
fn wallet_connector_rejects_mismatched_aggregate_keysets() {
    let bundle = bundle();
    let mut advertised = advertised_keyset(&bundle);
    advertised.keys = cdk_common::Keys::new(BTreeMap::new());

    assert!(matches!(
        bundle
            .public_config
            .validate_member_advertised_keysets(member_id(1), &[advertised],),
        Err(FederationError::Nut02(_) | FederationError::AdvertisedFederatedKeysetMismatch { .. })
    ));
}

#[test]
fn federated_mint_connector_rejects_invalid_member_call_policy() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);

    for config in [
        FederatedMintConnectorConfig {
            member_fallback_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            member_request_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_status_member_timeout: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            fanout_concurrency: 0,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_visibility_attempts: 0,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_visibility_retry_delay: Duration::ZERO,
            ..FederatedMintConnectorConfig::default()
        },
        FederatedMintConnectorConfig {
            quote_coordinator_member_id: Some(member_id(99)),
            ..FederatedMintConnectorConfig::default()
        },
    ] {
        assert!(FederatedMintConnector::new_with_config(
            bundle.public_config.clone(),
            connectors.clone(),
            config,
        )
        .is_err());
    }
}

#[test]
fn federated_mint_connector_rejects_tampered_aggregate_key() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);
    let mut public_config = bundle.public_config.clone();
    let amount = Amount::from(1);
    let wrong_aggregate_key = *public_config.keysets[0]
        .aggregate_keys
        .get(&Amount::from(2))
        .expect("alternate aggregate key");
    public_config.keysets[0]
        .aggregate_keys
        .insert(amount, wrong_aggregate_key);

    assert!(FederatedMintConnector::new(public_config, connectors).is_err());
}

#[test]
fn federated_mint_connector_rejects_tampered_public_config_urls() {
    let bundle = threshold_bundle();
    let public_config = public_config_with_mismatched_public_share(&bundle);

    assert!(FederatedMintConnector::from_public_config_urls(public_config).is_err());
}

#[test]
fn federated_mint_connector_trusted_public_config_urls_skip_full_aggregate_validation() {
    let bundle = threshold_bundle();
    let public_config = public_config_with_mismatched_public_share(&bundle);

    let connector = FederatedMintConnector::from_validated_public_config_urls_with_config(
        public_config.clone(),
        FederatedMintConnectorConfig::default(),
    )
    .expect("trusted local-registry config should skip repeated aggregate validation");

    assert_eq!(connector.public_config(), &public_config);
}

#[test]
fn federated_mint_connector_routes_only_exact_member_websocket_urls() {
    let bundle = threshold_bundle();
    let connector = FederatedMintConnector::from_validated_public_config_urls_with_config(
        bundle.public_config,
        FederatedMintConnectorConfig::default(),
    )
    .expect("validated federation connector");

    assert_eq!(
        connector.websocket_member_id("wss://mint1.example.com/v1/ws"),
        Some(member_id(1))
    );
    assert_eq!(
        connector.websocket_member_id("wss://mint1.example.com/v1/not-ws"),
        None
    );
    assert_eq!(
        connector.websocket_member_id("wss://unlisted.example.com/v1/ws"),
        None
    );

    let iroh_url = MintUrl::from_str(
        "iroh://2c3188b9f7ecfec6b5c765a71f85221bb6165f7ceaf6cc9fd4d7344b94401fc3",
    )
    .expect("Iroh mint URL");
    assert_eq!(
        member_websocket_url(&iroh_url)
            .expect("Iroh WebSocket URL")
            .as_str(),
        "iroh://2c3188b9f7ecfec6b5c765a71f85221bb6165f7ceaf6cc9fd4d7344b94401fc3/v1/ws"
    );
}

#[tokio::test]
async fn validated_config_url_members_skip_redundant_keyset_discovery() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_mint_keys_error("validated config should avoid key discovery");
        connector.set_post_mint_response(Ok(share_response(&bundle, member_id, &outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new_with_validated_config_and_member_cache(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig::default(),
        true,
    )
    .expect("connector with config-derived member cache");

    assert_eq!(
        connector
            .get_mint_keys()
            .await
            .expect("aggregate keys from validated config"),
        vec![advertised]
    );
    connector
        .get_mint_keyset(outputs[0].keyset_id)
        .await
        .expect("one aggregate keyset from validated config");
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
        .expect("mint response without key discovery");
    assert_aggregate_response_valid(&bundle, &outputs, &response);
}

#[tokio::test]
async fn config_derived_member_cache_fails_closed_and_clears_on_rotation() {
    let bundle = threshold_rotation_bundle();
    let rotated_config = rotated_public_config(&bundle);
    let rotated_keyset = rotated_config
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .map(super::super::validation::federated_keyset_to_keyset)
        .expect("rotated active keyset");
    let mut connectors = BTreeMap::new();
    let mut raw_connectors = Vec::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = Arc::new(MockMintConnector::new());
        connector.set_active_keyset(rotated_keyset.clone());
        connector.set_mint_info_response(Ok(MintInfo::new().federation(
            rotated_config
                .mint_info_for_member(member_id)
                .expect("rotated federation mint info"),
        )));
        raw_connectors.push((member_id, connector.clone()));
        connectors.insert(member_id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new_with_validated_config_and_member_cache(
        bundle.public_config.clone(),
        connectors,
        FederatedMintConnectorConfig::default(),
        true,
    )
    .expect("connector with stale config-derived member cache");

    let err = connector
        .get_mint_info()
        .await
        .expect_err("rotated member metadata must not match the imported config");
    assert!(err.to_string().contains("matching mint info"));

    for (member_id, connector) in raw_connectors {
        connector.set_mint_info_response(Ok(federation_mint_info(&bundle, member_id)));
        connector.set_mint_keys_error("cleared cache must perform live validation");
    }
    let err = connector
        .get_mint_keys()
        .await
        .expect_err("failed live validation must remain closed");
    assert!(err
        .to_string()
        .contains("insufficient federation keyset advertisements"));
}

#[test]
fn federated_mint_connector_builds_http_members_from_invite_code() {
    let bundle = threshold_bundle();
    let invite = FederationInvite::new(bundle.public_config.clone()).expect("invite");
    let invite_code = invite.to_invite_code().expect("invite code");
    let config = FederatedMintConnectorConfig {
        quote_coordinator_member_id: Some(member_id(2)),
        fanout_concurrency: 2,
        ..FederatedMintConnectorConfig::default()
    };

    let connector = FederatedMintConnector::from_invite_code_with_config(&invite_code, config)
        .expect("connector from invite");

    assert_eq!(connector.public_config(), &bundle.public_config);
    assert_eq!(connector.connector_config(), config);
    assert_eq!(connector.quote_coordinator_member_id, member_id(2));
    assert_eq!(
        connector
            .member_connectors
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![member_id(1), member_id(2), member_id(3)]
    );
}

#[test]
fn federated_mint_connector_rejects_invalid_invite_code() {
    assert!(FederatedMintConnector::from_invite_code("cashuAunsupported").is_err());
}

#[test]
fn federated_mint_connector_confirms_paid_websocket_melts() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");

    assert!(connector.confirm_paid_melt_notification());
}

#[test]
fn federated_mint_connector_rejects_unsupported_wallet_protocol_version() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connectors = member_connectors(&bundle, &outputs);
    let mut public_config = bundle.public_config.clone();
    let expected = public_config.wallet_protocol_version;
    let found = expected.saturating_add(1);
    public_config.wallet_protocol_version = found;

    assert!(matches!(
        FederatedMintConnector::new(public_config, connectors),
        Err(crate::Error::Federation(
            FederationError::ProtocolVersionMismatch {
                component: "wallet",
                expected: actual_expected,
                found: actual_found,
            }
        )) if actual_expected == expected && actual_found == found
    ));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_mint_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
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
