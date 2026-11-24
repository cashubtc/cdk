//! Aggregating mint, swap and melt-change signature shares, including keyset rotation.

use super::*;

#[tokio::test]
async fn federated_mint_connector_aggregates_fake_wallet_member_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        fake_wallet_member_connectors_with_melt_quote(
            &bundle,
            &quote_id,
            3,
            &melt_quote_id,
            &melt_invoice,
            1,
        )
        .await,
    )
    .expect("federated mint connector");

    let fake_quote = connector
        .post_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(3),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }))
        .await
        .expect("fake wallet mint quote");
    let fake_quote_id = fake_quote.quote().to_string();
    let mut settled_state = fake_quote.state();
    for _ in 0..100 {
        let status = connector
            .get_mint_quote_status(PaymentMethod::Known(KnownMethod::Bolt11), &fake_quote_id)
            .await
            .expect("fake wallet quote status");
        settled_state = status.state();
        if settled_state == Some(MintQuoteState::Paid) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(settled_state, Some(MintQuoteState::Paid));

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: quote_id.to_string(),
                outputs: outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &mint_response);

    let batch_response = connector
        .post_batch_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            BatchMintRequest {
                quotes: vec![fake_quote_id],
                quote_amounts: None,
                outputs: outputs.clone(),
                signatures: None,
            },
        )
        .await
        .expect("batch mint response");
    assert_aggregate_response_valid(&bundle, &outputs, &batch_response);

    let swap_outputs = blinded_outputs_for_keyset(&active_keyset(&bundle), b"fake-wallet-swap");
    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![
                proof_for_amount(&bundle, 1, "fake-wallet-swap-one"),
                proof_for_amount(&bundle, 2, "fake-wallet-swap-two"),
            ],
            swap_outputs.clone(),
        ))
        .await
        .expect("swap response");
    assert_aggregate_response_valid(
        &bundle,
        &swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let change_outputs = vec![blinded_change_outputs(&bundle)
        .into_iter()
        .next()
        .expect("change output")];
    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![proof_for_amount(&bundle, 4, "fake-wallet-melt")],
                Some(change_outputs.clone()),
            ),
        )
        .await
        .expect("melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_rotated_fake_wallet_member_shares() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let old_restore_output =
        blinded_output_for_keyset_amount(&old_keyset, 4, b"rotated wallet old restore");
    let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated wallet output");
    let next_swap_outputs = vec![
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated wallet swap output a"),
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated wallet swap output b"),
    ];
    let next_change_outputs =
        vec![
            blinded_change_outputs_for_keyset(&next_keyset, b"rotated wallet melt change")
                .into_iter()
                .next()
                .expect("next change output"),
        ];
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        rotated_fake_wallet_member_connectors_with_state(
            &bundle,
            &old_mint_quote_id,
            &new_mint_quote_id,
            &old_restore_output,
            &melt_quote_id,
            &melt_invoice,
            1,
        )
        .await,
    )
    .expect("rotated federated mint connector");

    let advertised = connector.get_mint_keys().await.expect("active keysets");
    assert_eq!(advertised.len(), 1);
    assert_eq!(advertised[0].id, next_keyset.id);

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: new_mint_quote_id.to_string(),
                outputs: next_outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("rotated mint response");
    assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-wallet-swap")],
            next_swap_outputs.clone(),
        ))
        .await
        .expect("rotated swap response");
    assert_aggregate_response_valid(
        &bundle,
        &next_swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![proof_for_amount(&bundle, 4, "rotated-wallet-melt")],
                Some(next_change_outputs.clone()),
            ),
        )
        .await
        .expect("rotated melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = next_change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: vec![old_restore_output.clone()],
        })
        .await
        .expect("rotated restore response");
    assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
    assert_aggregate_response_valid(
        &bundle,
        &[old_restore_output],
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_maps_rotated_stale_outputs_to_inactive_keyset() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let stale_outputs = vec![blinded_output_for_keyset_amount(
        &old_keyset,
        4,
        b"rotated stale wallet output",
    )];
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        member_connectors_with_only_keysets(&bundle),
    )
    .expect("rotated federated mint connector");

    let result = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-stale-swap")],
            stale_outputs,
        ))
        .await;

    assert!(
        matches!(result, Err(crate::Error::InactiveKeyset)),
        "stale rotated swap outputs should retry through InactiveKeyset, got {result:?}"
    );
}

#[tokio::test]
async fn federated_wallet_swaps_restored_old_keyset_proofs_after_rotation() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let seed = [42u8; 64];
    let restore_secrets =
        PreMintSecrets::restore_batch(old_keyset.id, &seed, 0, 1).expect("restore secrets");
    let restore_output = restore_secrets
        .blinded_messages()
        .into_iter()
        .next()
        .expect("restore output");
    let mut old_mint_output = restore_output.clone();
    old_mint_output.amount = Amount::from(4);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connector = Arc::new(
        FederatedMintConnector::new(
            rotated_public_config(&bundle),
            rotated_fake_wallet_member_connectors_with_state(
                &bundle,
                &old_mint_quote_id,
                &new_mint_quote_id,
                &old_mint_output,
                &melt_quote_id,
                &melt_invoice,
                1,
            )
            .await,
        )
        .expect("rotated federated mint connector"),
    );
    let first_member = bundle
        .public_config
        .members
        .values()
        .next()
        .expect("first member");
    let localstore = Arc::new(
        cdk_sqlite::wallet::memory::empty()
            .await
            .expect("wallet db"),
    );
    let wallet = WalletBuilder::new()
        .mint_url(first_member.public_mint_url.clone())
        .unit(CurrencyUnit::Sat)
        .localstore(localstore)
        .seed(seed)
        .shared_client(connector)
        .build()
        .expect("wallet");

    let restored = wallet
        .restore_with_opts(crate::wallet::NUT13Options::new(1, 1).expect("restore opts"))
        .await
        .expect("restore old-keyset proof");
    assert_eq!(restored.unspent, Amount::from(4));
    assert_eq!(
        wallet
            .active_keyset()
            .await
            .expect("active keyset after rotation")
            .id,
        next_keyset.id
    );
    let old_proofs = wallet.get_unspent_proofs().await.expect("old proofs");
    assert_eq!(
        old_proofs.total_amount().expect("old proof amount"),
        Amount::from(4)
    );

    wallet
        .swap(
            None,
            crate::amount::SplitTarget::default(),
            old_proofs.clone(),
            None,
            false,
            false,
        )
        .await
        .expect("swap restored old-keyset proofs");
    let spent_states = wallet
        .check_proofs_spent(old_proofs)
        .await
        .expect("spent states");
    assert!(spent_states
        .iter()
        .all(|proof_state| proof_state.state == State::Spent));
}

#[tokio::test]
async fn federated_mint_connector_aggregates_rotated_http_member_shares() {
    let bundle = threshold_rotation_bundle();
    let old_keyset = active_keyset(&bundle);
    let next_keyset = activated_rotation_keyset(&bundle);
    let old_mint_quote_id = QuoteId::new();
    let new_mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let old_restore_output =
        blinded_output_for_keyset_amount(&old_keyset, 4, b"rotated http old restore");
    let next_outputs = blinded_outputs_for_keyset(&next_keyset, b"rotated http output");
    let next_swap_outputs = vec![
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated http swap output a"),
        blinded_output_for_keyset_amount(&next_keyset, 2, b"rotated http swap output b"),
    ];
    let next_change_outputs =
        vec![
            blinded_change_outputs_for_keyset(&next_keyset, b"rotated http melt change")
                .into_iter()
                .next()
                .expect("next change output"),
        ];
    let local_connectors = rotated_fake_wallet_member_connectors_with_state(
        &bundle,
        &old_mint_quote_id,
        &new_mint_quote_id,
        &old_restore_output,
        &melt_quote_id,
        &melt_invoice,
        1,
    )
    .await;
    let connector = FederatedMintConnector::new(
        rotated_public_config(&bundle),
        http_member_connectors(local_connectors),
    )
    .expect("rotated federated HTTP mint connector");

    let advertised = connector
        .get_mint_keyset(next_keyset.id)
        .await
        .expect("next keyset over HTTP");
    assert_eq!(advertised.id, next_keyset.id);

    let mint_response = connector
        .post_mint(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MintRequest {
                quote: new_mint_quote_id.to_string(),
                outputs: next_outputs.clone(),
                signature: None,
            },
        )
        .await
        .expect("rotated HTTP mint response");
    assert_aggregate_response_valid(&bundle, &next_outputs, &mint_response);

    let swap_response = connector
        .post_swap(SwapRequest::new(
            vec![proof_for_amount(&bundle, 4, "rotated-http-swap")],
            next_swap_outputs.clone(),
        ))
        .await
        .expect("rotated HTTP swap response");
    assert_aggregate_response_valid(
        &bundle,
        &next_swap_outputs,
        &MintResponse {
            signatures: swap_response.signatures,
        },
    );

    let melt_proof = proof_for_amount(&bundle, 4, "rotated-http-melt");
    let melt_y = melt_proof.y().expect("melt proof y");
    let initial_melt_response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new(
                melt_quote_id.to_string(),
                vec![melt_proof],
                Some(next_change_outputs.clone()),
            ),
        )
        .await
        .expect("rotated HTTP melt response");
    let melt_response =
        settled_federation_melt_response(&connector, &melt_quote_id, initial_melt_response).await;
    let change = melt_response.change().cloned().expect("change signatures");
    let mut signed_change_output = next_change_outputs[0].clone();
    signed_change_output.amount = Amount::from(2);
    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    assert_aggregate_response_valid(
        &bundle,
        &[signed_change_output],
        &MintResponse { signatures: change },
    );
    let melt_status = connector
        .get_melt_quote_status(
            PaymentMethod::Known(KnownMethod::Bolt11),
            &melt_quote_id.to_string(),
        )
        .await
        .expect("rotated HTTP melt status");
    assert_eq!(melt_status.state(), MeltQuoteState::Paid);
    let spent_state = connector
        .post_check_state(CheckStateRequest { ys: vec![melt_y] })
        .await
        .expect("rotated HTTP check-state");
    assert_eq!(spent_state.states.len(), 1);
    assert_eq!(spent_state.states[0].state, State::Spent);

    let restore_response = connector
        .post_restore(RestoreRequest {
            outputs: vec![old_restore_output.clone()],
        })
        .await
        .expect("rotated HTTP restore response");
    assert_eq!(restore_response.outputs, vec![old_restore_output.clone()]);
    assert_aggregate_response_valid(
        &bundle,
        &[old_restore_output],
        &MintResponse {
            signatures: restore_response.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_preserves_threshold_inactive_keyset_rejections() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let outputs = blinded_outputs(&bundle);
    let mut connectors = BTreeMap::new();

    for member_id in [member_id(1), member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_swap_response(Err(crate::Error::InactiveKeyset));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let result = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs))
        .await;

    assert!(
        matches!(result, Err(crate::Error::InactiveKeyset)),
        "threshold inactive-keyset member rejections should remain retryable, got {result:?}"
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_swap_signature_shares() {
    let bundle = threshold_bundle();
    let outputs = blinded_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors(&bundle, &outputs),
    )
    .expect("federated mint connector");
    let response = connector
        .post_swap(SwapRequest::new(Vec::new(), outputs.clone()))
        .await
        .expect("swap response");

    assert_aggregate_response_valid(
        &bundle,
        &outputs,
        &MintResponse {
            signatures: response.signatures,
        },
    );
}

#[tokio::test]
async fn local_federated_members_reject_sliding_window_swap_output_attack() {
    let bundle = threshold_four_bundle();
    let keyset = active_keyset(&bundle);
    let outputs = [
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-a"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-b"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-c"),
        blinded_output_for_keyset_amount(&keyset, 1, b"sliding-window-swap-output-d"),
    ];
    let inputs = vec![
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-a"),
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-b"),
        proof_for_amount(&bundle, 1, "sliding-window-swap-input-c"),
    ];
    let mint_quote_id = QuoteId::new();
    let melt_quote_id = QuoteId::new();
    let melt_invoice = create_fake_invoice(1000, "".to_string());
    let connectors = fake_wallet_member_connectors_with_melt_quote(
        &bundle,
        &mint_quote_id,
        3,
        &melt_quote_id,
        &melt_invoice,
        1000,
    )
    .await;
    let views = [
        (
            member_id(1),
            vec![outputs[0].clone(), outputs[1].clone(), outputs[2].clone()],
        ),
        (
            member_id(2),
            vec![outputs[1].clone(), outputs[2].clone(), outputs[3].clone()],
        ),
        (
            member_id(3),
            vec![outputs[2].clone(), outputs[3].clone(), outputs[0].clone()],
        ),
        (
            member_id(4),
            vec![outputs[3].clone(), outputs[0].clone(), outputs[1].clone()],
        ),
    ];
    let mut successes = Vec::new();
    let mut failures = Vec::new();

    for (member_id, member_outputs) in views {
        let connector = connectors
            .get(&member_id)
            .expect("member connector")
            .clone();
        match connector
            .post_swap(SwapRequest::new(inputs.clone(), member_outputs.clone()))
            .await
        {
            Ok(response) => successes.push((member_id, member_outputs, response)),
            Err(err) => failures.push((member_id, err.to_string())),
        }
    }

    assert_eq!(
        successes.len(),
        1,
        "only the first consensus-accepted swap view may receive shares"
    );
    assert_eq!(
        failures.len(),
        3,
        "overlapping swap views must not receive enough shares to aggregate"
    );
    for (_, error) in &failures {
        assert!(
            error.contains("Token Already Spent")
                || error.contains("missing persisted federation signature share"),
            "losing sliding-window swap view should fail before returning shares: {error}"
        );
    }

    let (_, winning_outputs, _) = successes.first().expect("winning view");
    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let aggregate = connector
        .post_swap(SwapRequest::new(inputs, winning_outputs.clone()))
        .await
        .expect("accepted swap view should aggregate across members");

    assert_aggregate_response_valid(
        &bundle,
        winning_outputs,
        &MintResponse {
            signatures: aggregate.signatures,
        },
    );
}

#[tokio::test]
async fn federated_mint_connector_aggregates_melt_change_signature_shares() {
    let bundle = threshold_bundle();
    let change_outputs = blinded_change_outputs(&bundle);
    let connector = FederatedMintConnector::new(
        bundle.public_config.clone(),
        member_connectors_with_melt_change(&bundle, &change_outputs),
    )
    .expect("federated mint connector");
    let response = connector
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
    let change = response.change().cloned().expect("change signatures");

    assert_aggregate_response_valid(
        &bundle,
        &signed_change_outputs(&change_outputs),
        &MintResponse { signatures: change },
    );
}

#[tokio::test]
async fn federated_mint_connector_fans_out_melt_without_change() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let failing_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(
        &failing_member,
        &bundle,
        member_id(1),
        advertised.clone(),
    );
    failing_member
        .set_post_melt_response(Err(crate::Error::Custom("member unavailable".to_string())));
    connectors.insert(
        member_id(1),
        Arc::new(failing_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_melt_response(Ok(melt_response_without_change()));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let connector = FederatedMintConnector::new(bundle.public_config.clone(), connectors)
        .expect("federated mint connector");
    let response = connector
        .post_melt(
            &PaymentMethod::Known(KnownMethod::Bolt11),
            MeltRequest::new("melt-quote".to_string(), Vec::new(), None),
        )
        .await
        .expect("melt response");

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(response.change().is_none());
}

#[tokio::test]
async fn federated_mint_connector_returns_threshold_pending_without_slow_surplus_member() {
    let bundle = threshold_bundle();
    let advertised = advertised_keyset(&bundle);
    let mut connectors = BTreeMap::new();

    let slow_member = MockMintConnector::new();
    set_member_advertisement_with_keyset(&slow_member, &bundle, member_id(1), advertised.clone());
    slow_member.set_post_melt_delay(Duration::from_secs(30));
    slow_member.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(
        melt_response_without_change_with_state(MeltQuoteState::Pending),
    )));
    connectors.insert(
        member_id(1),
        Arc::new(slow_member) as Arc<dyn MintConnector + Send + Sync>,
    );

    for member_id in [member_id(2), member_id(3)] {
        let connector = MockMintConnector::new();
        set_member_advertisement_with_keyset(&connector, &bundle, member_id, advertised.clone());
        connector.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(
            melt_response_without_change_with_state(MeltQuoteState::Pending),
        )));
        connectors.insert(
            member_id,
            Arc::new(connector) as Arc<dyn MintConnector + Send + Sync>,
        );
    }

    let request = (0..128)
        .find_map(|attempt| {
            let request = MeltRequest::new(format!("melt-quote-{attempt}"), Vec::new(), None);
            let envelope = cdk_common::federation::FederationOperationEnvelope::new(
                bundle.public_config.federation_id,
                cdk_common::federation::FederationOperation::Melt(
                    cdk_common::federation::FederationMeltOperation {
                        request: request.clone(),
                    },
                ),
            );
            let operation_id = envelope.operation_id().expect("melt operation id");
            let selector = u64::from_be_bytes(
                operation_id.to_bytes()[..8]
                    .try_into()
                    .expect("operation selector bytes"),
            );
            (selector % 3 == 0).then_some(request)
        })
        .expect("request whose old roster selector points at the slow member");
    let connector = FederatedMintConnector::new(bundle.public_config, connectors)
        .expect("federated mint connector");
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        connector.post_melt(&PaymentMethod::Known(KnownMethod::Bolt11), request),
    )
    .await
    .expect("threshold pending response must not wait for the slow surplus member")
    .expect("melt response");

    assert_eq!(response.state(), MeltQuoteState::Pending);
}
