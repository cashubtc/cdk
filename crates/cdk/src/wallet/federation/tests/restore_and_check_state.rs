//! Restore and check-state threshold views.

use super::*;

#[tokio::test]
async fn federated_mint_connector_accepts_threshold_matching_restore_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        let member_outputs = if member_id.get() == 2 {
            restore_outputs[0..1].to_vec()
        } else {
            restore_outputs.clone()
        };
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &member_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = MockMintConnector::new();
    set_member_advertisement_with_keyset(&connector, &bundle, member_id(3), advertised);
    connector._set_restore_response(Ok(restore_response(
        &bundle,
        member_id(3),
        &restore_outputs,
    )));
    connectors.insert(
        member_id(3),
        Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore response");

    assert_eq!(response.outputs, restore_outputs);
    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&restore_outputs),
        &MintResponse {
            signatures: response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_rejects_restore_without_matching_threshold() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for (member_id, member_outputs) in [
        (member_id(1), restore_outputs.clone()),
        (member_id(2), restore_outputs[0..1].to_vec()),
        (member_id(3), Vec::new()),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector._set_restore_response(Ok(restore_response(&bundle, member_id, &member_outputs)));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector
        .post_restore(RestoreRequest {
            outputs: restore_outputs,
        })
        .await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_collects_threshold_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-y-2")),
    ];
    let expected = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &failing_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    failing_member
        .set_check_state_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_response(Ok(expected.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_check_state(CheckStateRequest { ys })
        .await
        .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_accepts_threshold_matching_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-lag-y-2")),
    ];
    let stale = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let expected = proof_state_response(&ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for (member_id, response) in [
        (member_id(1), expected.clone()),
        (member_id(2), stale),
        (member_id(3), expected.clone()),
    ] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_check_state(CheckStateRequest { ys })
        .await
        .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_polls_check_state_members_concurrently() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-parallel-y-2")),
    ];
    let expected = proof_state_response(&ys, &[State::Unspent, State::Spent]);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_check_state_barrier(barrier.clone());
        connector.set_check_state_response(Ok(expected.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        connector.post_check_state(CheckStateRequest { ys }),
    )
    .await
    .expect("check-state probes should run concurrently")
    .expect("check-state response");

    assert_eq!(response, expected);
}

#[tokio::test]
async fn federated_mint_connector_rejects_mismatched_check_state_views() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let ys = vec![
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-1")),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(b"check-state-mismatch-y-2")),
    ];
    let first = proof_state_response(&ys, &[State::Unspent, State::Pending]);
    let second = proof_state_response(&ys, &[State::Spent, State::Pending]);
    let mut connectors = BTreeMap::new();

    for (member_id, response) in [(member_id(1), first), (member_id(2), second)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_check_state_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = MockMintConnector::new();
    connector.set_active_keyset(advertised);
    connector.set_check_state_response(Ok(proof_state_response(
        &ys,
        &[State::Unspent, State::Spent],
    )));
    connectors.insert(
        member_id(3),
        Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
    );

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector.post_check_state(CheckStateRequest { ys }).await;

    assert!(matches!(result, Err(crate::Error::InvalidMintResponse(_))));
}

#[tokio::test]
async fn federated_mint_connector_sets_auth_wallet_on_all_members() {
    let bundle = threshold_bundle();
    let auth_wallet = test_auth_wallet().await;
    let mut mocks = BTreeMap::new();
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = Arc::new(MockMintConnector::new());
        mocks.insert(member_id, connector.clone());
        connectors.insert(member_id, connector as Arc<dyn MintConnector + Send + Sync>);
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector.set_auth_wallet(Some(auth_wallet.clone())).await;

    for member in mocks.values() {
        let stored = member.get_auth_wallet().await.expect("auth wallet");
        assert_eq!(stored.mint_url, auth_wallet.mint_url);
        assert_eq!(member.auth_wallet_set_count(), 1);
    }

    connector.set_auth_wallet(None).await;

    for member in mocks.values() {
        assert!(member.get_auth_wallet().await.is_none());
        assert_eq!(member.auth_wallet_set_count(), 2);
    }
}
