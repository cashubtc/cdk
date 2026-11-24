//! Mint and melt quote status quorums, and melt change recovery from status.

use super::*;

#[test]
fn mint_quote_status_quorum_ignores_member_update_times() {
    let mut first = mint_quote_status_response_for("mint-quote", MintQuoteState::Paid);
    let mut second = first.clone();
    first.updated_at = 100;
    second.updated_at = 101;
    let responses = vec![
        (member_id(1), MintQuoteResponse::Bolt11(first)),
        (member_id(2), MintQuoteResponse::Bolt11(second)),
    ];

    assert!(
        has_threshold_matching_mint_quote_status(&responses, 2),
        "responses that differ only in updated_at must form one quorum"
    );
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_batch_mint_quote_status() {
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

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member.set_batch_mint_quote_status_response(Ok(statuses));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_batch_mint_quote_status_response(Err(crate::Error::UnknownQuote));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .post_batch_check_mint_quote_status(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchCheckMintQuoteRequest {
                quotes: vec!["mint-quote-1".to_string(), "mint-quote-2".to_string()],
            },
        )
        .await
        .expect_err("one member must not decide batch mint quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test]
async fn federated_mint_connector_uses_threshold_melt_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = match melt_response_without_change() {
        MeltQuoteResponse::Bolt11(response) => response,
        _ => panic!("expected bolt11 melt response"),
    };
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    failing_member.set_active_keyset(advertised.clone());
    failing_member.set_melt_quote_status_response(Err(crate::Error::Custom(
        "first member unavailable".to_string(),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Ok(status.clone()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_waits_for_matching_melt_quote_status_quorum() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
        MeltQuoteState::Pending,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        connector.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
            MeltQuoteState::Paid,
        )));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_accepts_melt_quote_status_expiry_skew() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let stale_member = MockMintConnector::new();
    stale_member.set_active_keyset(advertised.clone());
    stale_member.set_melt_quote_status_response(Ok(melt_response_without_change_with_state(
        MeltQuoteState::Pending,
    )));
    connectors.insert(
        member_id(1),
        Arc::new(stale_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for (member_id, expiry) in [(member_id(2), 42), (member_id(3), 45)] {
        let connector = MockMintConnector::new();
        connector.set_active_keyset(advertised.clone());
        let mut response = melt_response_without_change_with_state(MeltQuoteState::Unpaid);
        response.expiry = expiry;
        connector.set_melt_quote_status_response(Ok(response));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Unpaid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn rebuilt_federated_connector_omits_unverified_melt_status_change_shares() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let keyset = &bundle.public_config.keysets[0];
    let wrong_change_outputs =
        blinded_change_outputs_for_keyset(keyset, b"wrong restarted melt change outputs");
    let connectors = [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            connector.set_active_keyset(advertised.clone());
            let status = match melt_response(&bundle, member_id, &wrong_change_outputs) {
                MeltQuoteResponse::Bolt11(response) => response,
                _ => panic!("expected bolt11 melt response"),
            };
            connector.set_melt_quote_status_response(Ok(status));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect();

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect("melt quote status");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(
        response.change().is_none(),
        "a rebuilt connector must not return shares it cannot bind to the original outputs"
    );
}

#[tokio::test]
async fn federated_melt_status_recovery_ignores_malformed_change_share() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();
    for id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response(&bundle, id, &change_outputs)));
        let mut status = match melt_response(&bundle, id, &change_outputs) {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        if id == member_id(3) {
            // The Byzantine member returns matching status metadata with a
            // malformed `c` on every change signature (wrong key kind: it
            // cannot even be parsed as a BLS share).
            for signature in status
                .change
                .iter_mut()
                .flat_map(|change| change.iter_mut())
            {
                signature.c = PublicKey::from_hex(
                    "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
                )
                .expect("secp256k1 public key");
            }
        }
        connector.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
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

    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect(
            "status recovery must ignore the invalid member and aggregate the honest threshold",
        );

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse {
            signatures: response.change().cloned().expect("change signatures"),
        },
    );
}

#[tokio::test]
async fn federated_melt_status_recovery_ignores_wrong_change_share() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let change_outputs = blinded_change_outputs(&bundle);
    let mut connectors = BTreeMap::new();
    for id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response(&bundle, id, &change_outputs)));
        let mut status = match melt_response(&bundle, id, &change_outputs) {
            MeltQuoteResponse::Bolt11(response) => response,
            _ => panic!("expected bolt11 melt response"),
        };
        if id == member_id(3) {
            // The Byzantine member returns matching status metadata with a
            // well-formed but wrong `c` on every change signature.
            for signature in status
                .change
                .iter_mut()
                .flat_map(|change| change.iter_mut())
            {
                signature.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
                    b"byzantine invalid change share".as_slice(),
                ));
            }
        }
        connector.set_melt_quote_status_response(Ok(status));
        connectors.insert(
            id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    connector
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

    let response = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect(
            "status recovery must ignore the invalid member and aggregate the honest threshold",
        );

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse {
            signatures: response.change().cloned().expect("change signatures"),
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_rejects_single_member_melt_quote_status() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let status = match melt_response_without_change() {
        MeltQuoteResponse::Bolt11(response) => response,
        _ => panic!("expected bolt11 melt response"),
    };
    let mut connectors = BTreeMap::new();

    let lone_member = MockMintConnector::new();
    lone_member.set_active_keyset(advertised.clone());
    lone_member.set_melt_quote_status_response(Ok(status));
    connectors.insert(
        member_id(1),
        Arc::new(lone_member) as Arc<dyn MintConnector + Send + Sync>,
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

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let error = connector
        .get_melt_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), "melt-quote")
        .await
        .expect_err("one member must not decide melt quote status");

    assert!(matches!(error, crate::Error::InvalidMintResponse(_)));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_restore_signature_shares() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let restore_outputs = blinded_change_outputs(&bundle);
    let connectors = [member_id(1), member_id(2), member_id(3)]
        .into_iter()
        .map(|member_id| {
            let connector = MockMintConnector::new();
            set_member_advertisement_with_keyset(
                &connector,
                &bundle,
                member_id,
                advertised.clone(),
            );
            connector._set_restore_response(Ok(restore_response(
                &bundle,
                member_id,
                &restore_outputs,
            )));
            (
                member_id,
                Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect();
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
