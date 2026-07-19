#[tokio::test]
async fn aleph_bft_live_four_member_one_offline_value_window_catches_up_without_duplicate_payment()
{
    run_aleph_bft_live_one_member_offline_value_window(4).await;
}

#[tokio::test]
async fn aleph_bft_live_five_member_one_offline_value_window_catches_up_without_duplicate_payment()
{
    run_aleph_bft_live_one_member_offline_value_window(5).await;
}

async fn run_aleph_bft_live_one_member_offline_value_window(member_count: u16) {
    assert!(matches!(member_count, 4 | 5));
    let connected_member_count = usize::from(member_count - 1);
    let required_observations = member_count - 1;
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let live_options = fast_aleph_bft_live_session_options_with_idle_tail(pump_config, 500);
    let payment_policy = FederationPaymentObservationPolicy::member_quorum(required_observations);
    let (mut session, make_payment_calls) =
        start_multi_member_aleph_bft_live_session_with_payment_policy(
            member_count,
            3,
            live_options.clone(),
            payment_policy,
        )
        .await
        .expect("multi-member live value-operation session");
    let runtime_config = session.states[0]
        .mint
        .federation_config()
        .expect("value-operation federation config")
        .clone();
    let signature_threshold =
        usize::from(session.public_config.threshold_params.signature_threshold);
    let mut expected_operation_ids = Vec::new();

    let offline_index = connected_member_count;
    let offline_member = session.private_configs[offline_index].member_id;
    *session.states[offline_index]
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *session.states[offline_index]
        .federation
        .aleph_bft_live_session
        .write()
        .await = None;
    session.transport.insert(
        session.federation_api_urls[offline_index].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(session.states[0].clone()),
    );
    let offline_handle = session._handles.remove(offline_index);
    drop(offline_handle);

    let mint_quote_id = QuoteId::new();
    let mint_quote_envelope = runtime_config.mint_quote_operation(
        mint_quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(4),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: mint_quote_id.to_string(),
            request: format!("invoice-{mint_quote_id}"),
            amount: Some(Amount::from(4)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    );
    let mint_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_quote_envelope)
            .await;
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            mint_quote_operation_id,
            "one-offline mint quote",
        )
        .await;
    }
    expected_operation_ids.push(mint_quote_operation_id);

    for observer in 1..=required_observations {
        let observation = mint_payment_observation_envelope_for_member(
            &session.states[0],
            &mint_quote_id,
            observer,
            4,
            "live-value-window-paid",
        );
        let operation_id =
            submit_live_operation_allowing_pending_timeout(&session.states[0], observation).await;
        for state in &session.states[..offline_index] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                &format!("one-offline mint payment observation {observer}"),
            )
            .await;
        }
        expected_operation_ids.push(operation_id);
    }

    let keyset = session.public_config.keysets[0].clone();
    let mint_secrets = (0..4)
        .map(|index| cdk::secret::Secret::new(format!("live value proof {index}")))
        .collect::<Vec<_>>();
    let mint_outputs = mint_secrets
        .iter()
        .map(|secret| {
            BlindedMessage::new(
                Amount::from(1),
                keyset.id,
                PublicKey::from(BlsG1PublicKey::hash_to_curve(secret.as_bytes())),
            )
        })
        .collect::<Vec<_>>();
    let mint_envelope = runtime_config
        .mint_operation(MintInput::Single(MintRequest {
            quote: mint_quote_id.clone(),
            outputs: mint_outputs.clone(),
            signature: None,
        }))
        .expect("baseline mint operation");
    let mint_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], mint_envelope).await;
    expected_operation_ids.push(mint_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, mint_operation_id, "baseline mint").await;
    }

    let mut proofs = Vec::new();
    for (index, (output, secret)) in mint_outputs.iter().zip(mint_secrets).enumerate() {
        proofs.push(
            proof_from_accepted_output(
                &session.states[..offline_index],
                signature_threshold,
                mint_operation_id,
                u32::try_from(index).expect("mint output index"),
                output,
                secret,
            )
            .await,
        );
    }

    let swap_input = proofs.remove(0);
    let swap_input_y = swap_input.y().expect("live swap input y");
    let swap_blinded_output = swap_output(&session.states[0], "one-offline-value-window");
    let swap_envelope = runtime_config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![swap_blinded_output.clone()],
        ))
        .expect("live swap operation");
    let swap_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], swap_envelope).await;
    expected_operation_ids.push(swap_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, swap_operation_id, "one-offline swap").await;
    }

    for state in &session.states[..offline_index] {
        let restored = state
            .mint
            .restore(RestoreRequest {
                outputs: vec![swap_blinded_output.clone()],
            })
            .await
            .expect("connected member restore");
        assert_eq!(restored.outputs, vec![swap_blinded_output.clone()]);
        assert_eq!(restored.signatures.len(), 1);
        let checked = state
            .mint
            .check_state(&CheckStateRequest {
                ys: vec![swap_input_y],
            })
            .await
            .expect("connected member check-state");
        assert_eq!(checked.states.len(), 1);
        assert_eq!(checked.states[0].state, cdk::nuts::State::Spent);
    }

    let melt_quote_id = QuoteId::new();
    let pending_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(
            1_000,
            serde_json::to_string(&pending_description).expect("pending melt description"),
        ),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let melt_quote_envelope =
        test_melt_quote_operation(&runtime_config, melt_quote_id.clone(), melt_quote_request);
    let melt_quote_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_quote_envelope)
            .await;
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            melt_quote_operation_id,
            "one-offline melt quote",
        )
        .await;
    }
    expected_operation_ids.push(melt_quote_operation_id);

    let melt_input_ys = proofs
        .iter()
        .map(|proof| proof.y().expect("live melt input y"))
        .collect::<Vec<_>>();
    let mut melt_change = swap_output(&session.states[0], "one-offline-melt-change");
    melt_change.amount = Amount::ZERO;
    let melt_envelope = runtime_config
        .melt_operation(cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            proofs.clone(),
            Some(vec![melt_change.clone()]),
        ))
        .expect("live melt operation");
    let payment_actor = melt_envelope
        .verify_authorization(&session.public_config)
        .expect("one-offline melt author");
    let payment_actor_index = session
        .public_config
        .members
        .keys()
        .position(|member_id| *member_id == payment_actor)
        .expect("melt payment actor index");
    assert_ne!(payment_actor, offline_member);
    let melt_operation_id =
        submit_live_operation_allowing_pending_timeout(&session.states[0], melt_envelope).await;
    expected_operation_ids.push(melt_operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(state, melt_operation_id, "one-offline melt").await;
    }

    let mut melt_payment_operation_ids = Vec::new();
    let actor_observation = submit_melt_payment_observation_from_backend(
        &session.states[payment_actor_index],
        &melt_quote_id,
    )
    .await;
    expected_operation_ids.push(actor_observation.operation_id);
    melt_payment_operation_ids.push(actor_observation.operation_id);
    for state in &session.states[..offline_index] {
        wait_for_applied_operation_with_context(
            state,
            actor_observation.operation_id,
            "one-offline author melt payment observation",
        )
        .await;
    }
    for observer_index in
        (0..connected_member_count).filter(|observer_index| *observer_index != payment_actor_index)
    {
        let observation = matching_melt_payment_observation_envelope(
            &session.states[observer_index],
            &actor_observation.observation,
        );
        let operation_id = submit_live_operation_allowing_pending_timeout(
            &session.states[observer_index],
            observation,
        )
        .await;
        for state in &session.states[..offline_index] {
            wait_for_applied_operation_with_context(
                state,
                operation_id,
                &format!("one-offline melt payment observation {observer_index}"),
            )
            .await;
        }
        expected_operation_ids.push(operation_id);
        melt_payment_operation_ids.push(operation_id);
    }
    assert_eq!(
        make_payment_calls[payment_actor_index].load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "accepted melt must retain exactly one outgoing payment actor"
    );
    assert_eq!(make_payment_calls[offline_index].load(Ordering::SeqCst), 0);

    for state in &session.states[..offline_index] {
        for operation_id in &melt_payment_operation_ids {
            wait_for_applied_operation_with_context(
                state,
                *operation_id,
                "one-offline melt payment observation",
            )
            .await;
        }
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("connected melt quote lookup")
            .expect("connected melt quote");
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_eq!(
            state
                .mint
                .localstore()
                .get_proofs_states(&melt_input_ys)
                .await
                .expect("connected melt proof states"),
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_payment_observations(
                    cdk::federation::FederationPaymentObservationContext::MeltQuote,
                    &melt_quote_id.to_string(),
                )
                .await
                .expect("connected melt payment observations")
                .len(),
            connected_member_count
        );
    }

    assert!(session.states[offline_index]
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(swap_operation_id)
        .await
        .expect("offline swap journal lookup")
        .is_none());
    session.states[offline_index]
        .mark_federation_catching_up(member_id(1))
        .await;
    session.states[offline_index]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect_err("offline value-operation member must remain fail-closed");

    session.transport.insert(
        session.federation_api_urls[offline_index].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(session.states[offline_index].clone()),
    );
    sync_and_certify_partitioned_member(&session, offline_index, live_options).await;

    for operation_id in &expected_operation_ids {
        wait_for_recorded_operation(&session.states[offline_index], *operation_id).await;
    }
    session.states[offline_index]
        .ensure_federation_ready_for_authoritative_read()
        .await
        .expect("caught-up value-operation member is ready");
    for (index, state) in session.states.iter().enumerate() {
        let stats = state
            .federation_sync_transition_stats()
            .await
            .expect("value-operation sync transition stats");
        if index == offline_index {
            assert!(stats.lagging > 0);
            assert!(stats.catching_up > 0);
        } else {
            assert_eq!(
                stats,
                crate::FederationSyncTransitionStats::default(),
                "only the intentionally offline member should enter catch-up"
            );
        }
    }
    assert_eq!(
        make_payment_calls
            .iter()
            .map(|calls| calls.load(Ordering::SeqCst))
            .sum::<usize>(),
        1,
        "catch-up replay must not duplicate the outgoing payment"
    );
    assert_eq!(
        make_payment_calls[offline_index].load(Ordering::SeqCst),
        0,
        "non-author lagging member must not pay while replaying melt history"
    );

    let caught_up = &session.states[offline_index];
    let restored = caught_up
        .mint
        .restore(RestoreRequest {
            outputs: vec![swap_blinded_output.clone()],
        })
        .await
        .expect("caught-up member restore");
    assert_eq!(restored.outputs, vec![swap_blinded_output.clone()]);
    assert_eq!(restored.signatures.len(), 1);
    let checked = caught_up
        .mint
        .check_state(&CheckStateRequest {
            ys: vec![swap_input_y],
        })
        .await
        .expect("caught-up member check-state");
    assert_eq!(checked.states.len(), 1);
    assert_eq!(checked.states[0].state, cdk::nuts::State::Spent);
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("caught-up melt proof states"),
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MintQuote,
                &mint_quote_id.to_string(),
            )
            .await
            .expect("caught-up mint payment observations")
            .len(),
        connected_member_count
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_payment_observations(
                cdk::federation::FederationPaymentObservationContext::MeltQuote,
                &melt_quote_id.to_string(),
            )
            .await
            .expect("caught-up melt payment observations")
            .len(),
        connected_member_count
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(swap_operation_id, 0)
            .await
            .expect("caught-up swap share provenance")
            .len(),
        1
    );
    assert_eq!(
        caught_up
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(melt_operation_id, 0)
            .await
            .expect("caught-up melt change share provenance")
            .first()
            .map(|record| record.blinded_secret),
        Some(melt_change.blinded_secret)
    );
    for operation_id in [mint_operation_id, swap_operation_id, melt_operation_id] {
        assert!(caught_up
            .mint
            .localstore()
            .get_completed_operation(&federation_completed_operation_uuid(operation_id))
            .await
            .expect("caught-up completed operation lookup")
            .is_some());
    }

    let expected_entries = session.states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("expected value-operation journal");
    assert_eq!(
        expected_entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>(),
        expected_operation_ids
    );
    let expected_digest = materialized_state_digest(&session.states[0], &expected_entries)
        .await
        .expect("expected value-operation state digest");
    let expected_cursor = session.states[0]
        .federation_aleph_bft_status()
        .await
        .expect("expected value-operation cursor")
        .next_item_index;
    for state in &session.states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged value-operation journal");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged value-operation digest"),
            expected_digest
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("converged value-operation cursor")
                .next_item_index,
            expected_cursor
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(swap_operation_id, 0)
                .await
                .expect("converged swap share provenance")
                .len(),
            1,
            "restore and catch-up must not duplicate swap signing shares"
        );
        assert_eq!(
            state
                .mint
                .localstore()
                .get_federation_signature_shares_for_output(melt_operation_id, 0)
                .await
                .expect("converged melt share provenance")
                .len(),
            1,
            "catch-up must not duplicate melt change signing shares"
        );
        for output_index in 0..4 {
            assert_eq!(
                state
                    .mint
                    .localstore()
                    .get_federation_signature_shares_for_output(mint_operation_id, output_index,)
                    .await
                    .expect("converged mint share provenance")
                    .len(),
                1,
                "catch-up must regenerate exactly one mint share per output"
            );
        }
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("converged pending operations")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_keyset_rotation_with_one_unavailable_member() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
    ];
    let threshold_params = ThresholdParams::with_bft_consensus(4, 3).expect("threshold params");
    let active_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[41 + index as u8; 32]))
        .collect::<Vec<_>>();
    let inactive_coefficients = (0..threshold_params.signature_threshold)
        .map(|index| BlsSecretKey::from_reduced_bytes(&[51 + index as u8; 32]))
        .collect::<Vec<_>>();
    let active_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        BTreeMap::from([(Amount::from(1), active_coefficients)]),
    );
    let inactive_keyset = TrustedDealerKeysetSpec::new(
        CurrencyUnit::Sat,
        false,
        0,
        None,
        BTreeMap::from([(Amount::from(1), inactive_coefficients)]),
    );
    let (public_config, private_configs) =
        transport_config_with_member_federation_api_urls_and_keysets(
            threshold_params,
            federation_api_urls.clone(),
            vec![active_keyset, inactive_keyset],
        );
    let old_keyset_id = public_config
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("active keyset")
        .id;
    let mut next_keyset = public_config
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive keyset")
        .clone();
    next_keyset.active = true;

    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
            )
            .await,
        );
    }

    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(30),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    transport.insert(
        federation_api_urls[1].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    let mut live_session_handles = Vec::new();
    for index in [0_usize, 2, 3] {
        let options = fast_aleph_bft_live_session_options(pump_config, 500);
        let handle = start_federation_aleph_bft_live_session_with_options_and_transport(
            &mut states[index],
            sync_config,
            options,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 3);

    for index in [0_usize, 2, 3] {
        states[index].federation.peer_sync_config = Some(sync_config);
    }

    let runtime_config =
        cdk::mint::FederationRuntimeConfig::new(public_config.clone(), private_configs[0].clone())
            .expect("runtime config");
    let envelope = runtime_config.operation_envelope(FederationOperation::KeysetRotation(
        FederationKeysetRotationOperation {
            keyset: next_keyset.clone(),
        },
    ));
    let operation_id = envelope.operation_id().expect("operation id");
    let first_envelope = envelope.clone();
    let third_envelope = envelope.clone();
    let fourth_envelope = envelope;
    let (first, third, fourth) = tokio::join!(
        states[0].submit_federation_envelope_to_consensus(first_envelope),
        states[2].submit_federation_envelope_to_consensus(third_envelope),
        states[3].submit_federation_envelope_to_consensus(fourth_envelope)
    );
    let responses = [first, third, fourth];
    if responses.iter().any(Result::is_err) {
        let errors = responses
            .iter()
            .filter_map(|response| response.as_ref().err())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if responses.iter().filter(|response| response.is_ok()).count() < 3 {
            let mut network_pairs = BTreeMap::<(MemberId, MemberId), (usize, usize)>::new();
            for request in transport.aleph_bft_network_message_requests() {
                let encoded_bytes = request.payload.encoded_message.len() / 2;
                let entry = network_pairs
                    .entry((request.sender, request.receiver))
                    .or_default();
                entry.0 = entry.0.saturating_add(1);
                entry.1 = entry.1.max(encoded_bytes);
            }
            let mut diagnostics = Vec::new();
            for index in [0_usize, 2, 3] {
                let status = states[index]
                    .federation_aleph_bft_status()
                    .await
                    .expect("AlephBFT status");
                let entries = states[index]
                    .mint
                    .localstore()
                    .get_federation_journal_entries()
                    .await
                    .expect("journal lookup");
                let pending = states[index]
                    .mint
                    .localstore()
                    .get_federation_aleph_bft_pending_operations(0)
                    .await
                    .expect("pending operation lookup");
                let entry_ids = entries
                    .iter()
                    .map(|entry| (entry.operation_id, entry.status()))
                    .collect::<Vec<_>>();
                let pending_ids = pending
                    .iter()
                    .map(|pending| pending.operation_id)
                    .collect::<Vec<_>>();
                diagnostics.push(format!(
                    "member {index}: status={status:?}, entries={entry_ids:?}, pending={pending_ids:?}"
                ));
            }
            panic!(
                "three live members should finalize keyset rotation: {}; diagnostics: {}; network_pairs={network_pairs:?}",
                errors.join("; "),
                diagnostics.join(" | ")
            );
        }
    }
    let response = responses
        .into_iter()
        .find_map(Result::ok)
        .expect("at least one keyset rotation response");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for index in [0_usize, 2, 3] {
        wait_for_applied_operation(&states[index], operation_id).await;
        let effective = states[index]
            .mint
            .effective_federation_config()
            .await
            .expect("effective federation config")
            .expect("federation config");
        assert!(
            effective
                .public_config()
                .keysets
                .iter()
                .any(|keyset| keyset.id == next_keyset.id && keyset.active),
            "rotated keyset is active for member {index}"
        );
        assert!(
            effective
                .public_config()
                .keysets
                .iter()
                .any(|keyset| keyset.id == old_keyset_id && !keyset.active),
            "old keyset is retired for member {index}"
        );
        assert!(states[index]
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after keyset rotation")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_sessions_finalize_multiple_pending_operations_after_long_unavailable_window(
) {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut states = Vec::new();
    for private_config in private_configs {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                db,
                &mnemonic.to_seed_normalized(""),
            )
            .await,
        );
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let mut pending_operations = Vec::new();
    for _ in 0..3 {
        let quote_id = QuoteId::new();
        let envelope = mint_quote_envelope(&states[0], &quote_id);
        let operation_id = envelope.operation_id().expect("operation id");
        let err = states[0]
            .submit_federation_envelope_to_consensus(envelope)
            .await
            .expect_err("long unavailable window should leave operation pending");
        assert_pending_finalization_error(&err, operation_id, 250);
        pending_operations.push((quote_id, operation_id));
    }

    let pending_ids = wait_for_aleph_bft_pending_operations(&states[0], 0)
        .await
        .into_iter()
        .map(|pending| pending.operation_id)
        .collect::<Vec<_>>();
    for (_, operation_id) in &pending_operations {
        assert!(
            pending_ids.contains(operation_id),
            "pending operation {operation_id} was not durably recorded"
        );
    }
    for state in states.iter().take(4) {
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            0
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup before returned member")
            .is_empty());
        for (quote_id, _) in &pending_operations {
            assert!(state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await
                .expect("quote lookup before returned member")
                .is_none());
        }
    }

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut states[4],
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    let expected_operation_ids = pending_operations
        .iter()
        .map(|(_, operation_id)| *operation_id)
        .collect::<Vec<_>>();
    for state in &states {
        for (_, operation_id) in &pending_operations {
            wait_for_applied_operation(state, *operation_id).await;
        }
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after returned member");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            expected_operation_ids
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            expected_operation_ids.len() as u64
        );
        for (quote_id, _) in &pending_operations {
            assert!(state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await
                .expect("quote lookup after returned member")
                .is_some());
        }
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after returned member")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_replays_pending_operation_when_restart_precedes_backup() {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let seed = Mnemonic::generate(12)
            .expect("mnemonic")
            .to_seed_normalized("");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed);
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[0].clone()),
    );

    for index in 0..4 {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }

    states[0].federation.peer_sync_config = Some(FederationPeerSyncConfig {
        operation_submission_timeout: Duration::from_millis(250),
        ..FederationPeerSyncConfig::default()
    });
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&states[0], &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let err = states[0]
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("unavailable member should leave operation pending first");
    assert_pending_finalization_error(&err, operation_id, 250);

    assert_eq!(
        wait_for_aleph_bft_pending_operations(&states[0], 0)
            .await
            .iter()
            .map(|pending| pending.operation_id)
            .collect::<Vec<_>>(),
        vec![operation_id]
    );
    let backup_bytes = states[0]
        .mint
        .localstore()
        .get_federation_aleph_bft_backup_bytes(0)
        .await
        .expect("AlephBFT backup lookup before restart")
        .unwrap_or_default();
    assert!(
        backup_bytes.is_empty(),
        "this regression covers the restart window before upstream AlephBFT backup bytes exist"
    );

    *states[0].federation.aleph_bft_network_inbound.write().await = None;
    *states[0].federation.aleph_bft_live_session.write().await = None;
    live_session_handles.remove(0);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
    let restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );
    states[0] = restarted_first_state;
    live_session_handles.push(restarted_first_handle);

    let fifth_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut states[4],
        FederationPeerSyncConfig {
            operation_submission_timeout: Duration::from_secs(10),
            ..sync_config
        },
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start returned member live session")
    .expect("returned member live session handle");
    live_session_handles.push(fifth_handle);
    transport.insert(
        federation_api_urls[4].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(states[4].clone()),
    );
    assert_eq!(live_session_handles.len(), 5);

    for (index, state) in states.iter().enumerate() {
        if !operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
            let status = state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status");
            let entries = state
                .mint
                .localstore()
                .get_federation_journal_entries()
                .await
                .expect("journal lookup");
            let pending = state
                .mint
                .localstore()
                .get_federation_aleph_bft_pending_operations(0)
                .await
                .expect("pending operation lookup");
            panic!(
                "member {index} did not apply restarted AlephBFT operation; status={status:?}; journal_entries={}; pending_operations={}",
                entries.len(),
                pending.len()
            );
        }
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            1
        );
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after restarted finality")
            .is_empty());
    }

    let expected_entries = states[0]
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("unit-creation crash-window journal");
    assert_eq!(expected_entries.len(), 1);
    assert_eq!(expected_entries[0].operation_id, operation_id);
    let expected_digest = materialized_state_digest(&states[0], &expected_entries)
        .await
        .expect("unit-creation crash-window state digest");
    for state in &states {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("converged unit-creation crash-window journal");
        assert_eq!(
            journal_convergence_projection(&entries),
            journal_convergence_projection(&expected_entries)
        );
        assert_eq!(
            materialized_state_digest(state, &entries)
                .await
                .expect("converged unit-creation crash-window state digest"),
            expected_digest
        );
    }
}

#[tokio::test]
async fn aleph_bft_live_session_restarts_from_persisted_backup_and_continues_finality() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let second_seed = Mnemonic::generate(12)
        .expect("second mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    assert!(!wait_for_aleph_bft_backup_bytes(&first_state, 0)
        .await
        .is_empty());

    *first_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *first_state.federation.aleph_bft_live_session.write().await = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        first_private,
        Arc::clone(&first_db),
        &first_seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
    let _restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&restarted_first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = restarted_first_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for state in [&restarted_first_state, &second_state] {
        wait_for_applied_operation(state, first_operation_id).await;
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after post-backup restart");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].operation_id, first_operation_id);
        assert_eq!(entries[1].operation_id, second_operation_id);
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&first_quote_id)
            .await
            .expect("first quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_mint_quote(&second_quote_id)
            .await
            .expect("second quote lookup")
            .is_some());
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after post-backup restart")
            .is_empty());
    }
}

#[tokio::test]
async fn aleph_bft_live_session_replays_finalized_operation_from_backup_without_local_apply() {
    let first_url = MintUrl::from_str("http://fed1.test").expect("first federation api url");
    let second_url = MintUrl::from_str("http://fed2.test").expect("second federation api url");
    let (public_config, first_private, second_private) =
        transport_config_with_federation_api_urls(first_url.clone(), second_url.clone());
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_seed = Mnemonic::generate(12)
        .expect("first mnemonic")
        .to_seed_normalized("");
    let second_seed = Mnemonic::generate(12)
        .expect("second mnemonic")
        .to_seed_normalized("");
    let mut first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_seed,
    )
    .await;
    let mut second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private,
        second_db,
        &second_seed,
    )
    .await;
    let transport = RouterFederationTransportRegistry::default();
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(1),
        operation_submission_timeout: Duration::from_secs(10),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 128,
        max_peer_deliveries_per_message: 128,
    };

    let first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start first live session")
    .expect("first live session handle");
    transport.insert(
        first_url.clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(first_state.clone()),
    );
    let _second_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut second_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("start second live session")
    .expect("second live session handle");
    transport.insert(
        second_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(second_state.clone()),
    );

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&first_state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = first_state
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first operation finality before crash-window recovery");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    wait_for_applied_operation(&first_state, first_operation_id).await;
    wait_for_applied_operation(&second_state, first_operation_id).await;
    let backup_bytes = wait_for_aleph_bft_backup_bytes(&first_state, 0).await;

    *first_state
        .federation
        .aleph_bft_network_inbound
        .write()
        .await = None;
    *first_state.federation.aleph_bft_live_session.write().await = None;
    drop(first_handle);
    transport.insert(
        first_url.clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(second_state.clone()),
    );

    let crash_window_db = Arc::new(
        cdk_sqlite::mint::memory::empty()
            .await
            .expect("crash-window first db"),
    );
    let mut recovered_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        first_private,
        Arc::clone(&crash_window_db),
        &first_seed,
    )
    .await;
    let mut tx = recovered_first_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin crash-window backup transaction");
    tx.append_federation_aleph_bft_backup_bytes(0, &backup_bytes)
        .await
        .expect("copy upstream AlephBFT backup bytes");
    tx.set_federation_aleph_bft_active_session(0)
        .await
        .expect("persist crash-window active session");
    tx.commit()
        .await
        .expect("commit crash-window backup transaction");

    assert!(recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal before backup-only recovery")
        .is_empty());
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items before backup-only recovery")
        .is_empty());
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("quote before backup-only recovery")
        .is_none());

    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&recovered_first_state.mint)
            .await
            .expect("replay backup-only AlephBFT runtime");
    recovered_first_state.federation.consensus = replayed_consensus;
    recovered_first_state.federation.sequences = replayed_sequences;
    let recovered_status = recovered_first_state
        .federation_aleph_bft_status()
        .await
        .expect("backup-only AlephBFT status before live start");
    assert_eq!(recovered_status.session, 0);
    assert_eq!(recovered_status.finalized_item_count, 0);

    let _recovered_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut recovered_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart backup-only first live session")
    .expect("backup-only first live session handle");
    transport.insert(
        first_url,
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(recovered_first_state.clone()),
    );

    wait_for_applied_operation(&recovered_first_state, first_operation_id).await;
    assert!(recovered_first_state
        .mint
        .localstore()
        .get_mint_quote(&first_quote_id)
        .await
        .expect("quote after backup-only recovery")
        .is_some());
    let recovered_entries = recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal after backup-only recovery");
    assert_eq!(recovered_entries.len(), 1);
    assert_eq!(recovered_entries[0].operation_id, first_operation_id);
    let peer_entries = second_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal after backup-only recovery");
    assert_eq!(
        journal_convergence_projection(&recovered_entries),
        journal_convergence_projection(&peer_entries)
    );
    assert_eq!(
        materialized_state_digest(&recovered_first_state, &recovered_entries)
            .await
            .expect("recovered backup-only state digest"),
        materialized_state_digest(&second_state, &peer_entries)
            .await
            .expect("peer backup-only state digest")
    );

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&recovered_first_state, &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = recovered_first_state
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second operation finality after backup-only recovery");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for state in [&recovered_first_state, &second_state] {
        wait_for_applied_operation(state, second_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal after backup-only continuation");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status after backup-only continuation")
                .finalized_item_count,
            2
        );
    }
    let expected_entries = second_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("peer journal after backup-only continuation");
    let expected_digest = materialized_state_digest(&second_state, &expected_entries)
        .await
        .expect("peer state digest after backup-only continuation");
    let recovered_entries = recovered_first_state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("recovered journal after backup-only continuation");
    assert_eq!(
        journal_convergence_projection(&recovered_entries),
        journal_convergence_projection(&expected_entries)
    );
    assert_eq!(
        materialized_state_digest(&recovered_first_state, &recovered_entries)
            .await
            .expect("recovered state digest after backup-only continuation"),
        expected_digest
    );
}

#[tokio::test]
async fn aleph_bft_live_five_member_sessions_continue_after_post_backup_restart_with_multiple_operations(
) {
    let federation_api_urls = vec![
        MintUrl::from_str("http://fed1.test").expect("first federation api url"),
        MintUrl::from_str("http://fed2.test").expect("second federation api url"),
        MintUrl::from_str("http://fed3.test").expect("third federation api url"),
        MintUrl::from_str("http://fed4.test").expect("fourth federation api url"),
        MintUrl::from_str("http://fed5.test").expect("fifth federation api url"),
    ];
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(5, 3).expect("threshold params"),
        federation_api_urls.clone(),
    );
    let mut dbs = Vec::new();
    let mut seeds = Vec::new();
    let mut states = Vec::new();
    for private_config in private_configs.iter().cloned() {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let seed = Mnemonic::generate(12)
            .expect("mnemonic")
            .to_seed_normalized("");
        states.push(
            federation_route_state_for_private_with_db_and_aleph_bft(
                public_config.clone(),
                private_config,
                Arc::clone(&db),
                &seed,
            )
            .await,
        );
        dbs.push(db);
        seeds.push(seed);
    }
    let transport = RouterFederationTransportRegistry::default()
        .with_missing_router_wait(Duration::from_secs(1));
    let sync_config = FederationPeerSyncConfig {
        peer_sync_interval: Duration::from_millis(10),
        peer_request_timeout: Duration::from_secs(2),
        operation_submission_timeout: Duration::from_secs(15),
        ..FederationPeerSyncConfig::default()
    };
    let pump_config = CdkAlephBftNetworkPumpConfig {
        session_id: 0,
        max_messages_per_tick: 256,
        max_peer_deliveries_per_message: 256,
    };
    let mut live_session_handles = Vec::new();
    for index in 0..states.len() {
        let handle = start_federation_aleph_bft_live_session_with_transport(
            &mut states[index],
            sync_config,
            pump_config,
            transport.clone(),
        )
        .await
        .expect("start live session")
        .expect("live session handle");
        live_session_handles.push(handle);
        transport.insert(
            federation_api_urls[index].clone(),
            Router::new()
                .nest("/federation/v1", create_federation_router())
                .with_state(states[index].clone()),
        );
    }
    assert_eq!(live_session_handles.len(), 5);

    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&states[0], &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(first_envelope)
        .await
        .expect("first five-member operation finality before restart");
    assert_eq!(response.operation_id, first_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    for state in &states {
        wait_for_applied_operation(state, first_operation_id).await;
    }
    let backup_before_restart = wait_for_aleph_bft_backup_bytes(&states[0], 0).await;

    *states[0].federation.aleph_bft_network_inbound.write().await = None;
    *states[0].federation.aleph_bft_live_session.write().await = None;
    let first_handle = live_session_handles.remove(0);
    drop(first_handle);
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE })
            .with_state(states[1].clone()),
    );

    let mut restarted_first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        private_configs[0].clone(),
        Arc::clone(&dbs[0]),
        &seeds[0],
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_first_state.mint)
            .await
            .expect("replay restarted first AlephBFT runtime");
    restarted_first_state.federation.consensus = replayed_consensus;
    restarted_first_state.federation.sequences = replayed_sequences;
    let restarted_status = restarted_first_state
        .federation_aleph_bft_status()
        .await
        .expect("restarted AlephBFT status before live start");
    assert_eq!(restarted_status.finalized_item_count, 1);
    assert_eq!(restarted_status.next_unit_index, 1);
    let restarted_first_handle = start_federation_aleph_bft_live_session_with_transport(
        &mut restarted_first_state,
        sync_config,
        pump_config,
        transport.clone(),
    )
    .await
    .expect("restart first live session")
    .expect("restarted first live session handle");
    transport.insert(
        federation_api_urls[0].clone(),
        Router::new()
            .nest("/federation/v1", create_federation_router())
            .with_state(restarted_first_state.clone()),
    );
    states[0] = restarted_first_state;
    live_session_handles.push(restarted_first_handle);
    assert_eq!(live_session_handles.len(), 5);

    let second_quote_id = QuoteId::new();
    let second_envelope = mint_quote_envelope(&states[0], &second_quote_id);
    let second_operation_id = second_envelope.operation_id().expect("second operation id");
    let response = states[0]
        .submit_federation_envelope_to_consensus(second_envelope)
        .await
        .expect("second five-member operation finality after restart");
    assert_eq!(response.operation_id, second_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let backup_after_restart =
        wait_for_aleph_bft_backup_bytes_longer_than(&states[0], 0, backup_before_restart.len())
            .await;
    for state in &states {
        wait_for_applied_operation(state, second_operation_id).await;
    }

    let third_quote_id = QuoteId::new();
    let third_envelope = mint_quote_envelope(&states[1], &third_quote_id);
    let third_operation_id = third_envelope.operation_id().expect("third operation id");
    let response = states[1]
        .submit_federation_envelope_to_consensus(third_envelope)
        .await
        .expect("third five-member operation finality after restart");
    assert_eq!(response.operation_id, third_operation_id);
    assert_eq!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for state in &states {
        wait_for_applied_operation(state, third_operation_id).await;
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup after five-member post-backup restart");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.operation_id)
                .collect::<Vec<_>>(),
            vec![first_operation_id, second_operation_id, third_operation_id]
        );
        assert_eq!(
            state
                .federation_aleph_bft_status()
                .await
                .expect("AlephBFT status")
                .finalized_item_count,
            3
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_aleph_bft_pending_operations(0)
            .await
            .expect("pending operation lookup after five-member post-backup restart")
            .is_empty());
    }
    assert!(
        backup_after_restart.len() > backup_before_restart.len(),
        "restarted member should append new upstream AlephBFT backup bytes after finalizing another local unit"
    );
}

async fn wait_for_applied_operation(state: &MintState, operation_id: FederationOperationId) {
    assert!(
        operation_applied_within(state, operation_id, Duration::from_secs(15)).await,
        "operation applied before timeout"
    );
}

async fn wait_for_recorded_operation(state: &MintState, operation_id: FederationOperationId) {
    operation_terminal_within(state, operation_id, Duration::from_secs(15))
        .await
        .expect("operation recorded before timeout");
}

fn journal_convergence_projection(
    entries: &[FederationJournalEntry],
) -> Vec<(
    u64,
    FederationOperationId,
    FederationJournalDigest,
    FederationJournalDigest,
    FederationOperationStatus,
)> {
    entries
        .iter()
        .map(|entry| {
            (
                entry.index,
                entry.operation_id,
                entry.previous_order_digest,
                entry.order_digest,
                entry.status(),
            )
        })
        .collect()
}

async fn wait_for_terminal_operation(
    state: &MintState,
    operation_id: FederationOperationId,
) -> FederationOperationStatus {
    operation_terminal_within(state, operation_id, Duration::from_secs(30))
        .await
        .expect("operation reached terminal status before timeout")
}

async fn replay_aleph_bft_runtime_after_test_process_restart(state: &mut MintState) {
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("replay AlephBFT runtime after test process restart");
    state.federation.consensus = replayed_consensus;
    state.federation.sequences = replayed_sequences;
}

async fn submit_live_operation_allowing_pending_timeout(
    state: &MintState,
    envelope: FederationOperationEnvelope,
) -> FederationOperationId {
    let operation_id = envelope.operation_id().expect("live operation id");
    match state
        .submit_federation_envelope_to_consensus(envelope)
        .await
    {
        Ok(response) => assert_eq!(response.operation_id, operation_id),
        Err(err) => assert_pending_finalization_error(&err, operation_id, 10_000),
    }

    operation_id
}

async fn wait_for_applied_operation_with_context(
    state: &MintState,
    operation_id: FederationOperationId,
    context: &str,
) {
    if operation_applied_within(state, operation_id, Duration::from_secs(15)).await {
        return;
    }

    let member_id = state
        .mint
        .federation_config()
        .expect("contextual apply wait federation config")
        .member_id();
    let status = state.federation_aleph_bft_status().await;
    let journal_operation_ids = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("contextual apply wait journal")
        .into_iter()
        .map(|entry| entry.operation_id)
        .collect::<Vec<_>>();
    let pending_operation_ids = state
        .mint
        .localstore()
        .get_federation_aleph_bft_pending_operations(0)
        .await
        .expect("contextual apply wait pending operations")
        .into_iter()
        .map(|operation| operation.operation_id)
        .collect::<Vec<_>>();
    panic!(
        "{context}: operation {operation_id} did not apply on member {member_id} before timeout; status={status:?}; journal_operation_ids={journal_operation_ids:?}; pending_operation_ids={pending_operation_ids:?}"
    );
}

async fn sync_and_certify_partitioned_member(
    session: &MultiMemberAlephBftLiveSession,
    partitioned_index: usize,
    recovery_live_options: FederationAlephBftLiveSessionOptions,
) {
    let state = &session.states[partitioned_index];
    let public_config = &session.public_config;
    let clients = session
        .private_configs
        .iter()
        .cloned()
        .map(|private_config| {
            FederationRestPeerClient::new_with_config(
                session.transport.clone(),
                public_config,
                private_config,
                session.sync_config,
            )
            .expect("partition recovery peer-sync client")
        })
        .collect::<Vec<_>>();
    let peer_sets = session
        .private_configs
        .iter()
        .map(|private_config| {
            public_config
                .members
                .values()
                .filter(|member| member.member_id != private_config.member_id)
                .cloned()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let required_remote_quorum =
        usize::from(public_config.threshold_params.consensus_threshold) - 1;
    let catch_up_deadline = time::Instant::now() + Duration::from_secs(90);
    let mut maximum_synced = 0;
    let mut restarted_with_fast_test_options = false;
    loop {
        for index in 0..session.states.len() {
            let synced = run_federation_peer_sync_round(
                &session.states[index],
                &clients[index],
                &peer_sets[index],
            )
            .await;
            if index == partitioned_index {
                maximum_synced = maximum_synced.max(synced);
            }
        }
        if !restarted_with_fast_test_options
            && state.federation.live_session_active().await
            && matches!(
                state.federation_sync_status().await,
                Some(crate::FederationSyncStatus::CatchingUp { .. })
            )
        {
            let local_status = local_peer_status(state)
                .await
                .expect("partition recovery local status");
            let mut peer_statuses = Vec::new();
            for peer in &peer_sets[partitioned_index] {
                peer_statuses.push(
                    clients[partitioned_index]
                        .peer_status(peer)
                        .await
                        .expect("partition recovery peer status"),
                );
            }
            let target_session = sync_loop::quorum_compatible_session(
                &local_status,
                &peer_statuses,
                usize::from(public_config.threshold_params.consensus_threshold),
            )
            .expect("partition recovery quorum-compatible session");
            let replay_target = sync_loop::quorum_supported_replay_target(
                &local_status,
                &peer_statuses,
                target_session,
                usize::from(public_config.threshold_params.consensus_threshold),
            );
            stop_current_federation_aleph_bft_session_for_catch_up(state).await;
            start_federation_aleph_bft_live_session_with_options_and_transport(
                state,
                session.sync_config,
                recovery_live_options
                    .clone()
                    .with_replay_target_next_unit_index_by_member(replay_target),
                session.transport.clone(),
            )
            .await
            .expect("restart partition recovery with fast local test options")
            .expect("partition recovery fast live session");
            restarted_with_fast_test_options = true;
        }
        match state.ensure_federation_ready_for_authoritative_read().await {
            Ok(()) => {
                assert!(
                    maximum_synced >= required_remote_quorum,
                    "catch-up certification must reach a remote BFT quorum"
                );
                return;
            }
            Err(_) if time::Instant::now() < catch_up_deadline => {
                time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => {
                let local_status = local_peer_status(state)
                    .await
                    .expect("partition recovery timeout status");
                let trusted_checkpoint = state
                    .mint
                    .localstore()
                    .get_trusted_federation_checkpoint_quorum()
                    .await
                    .expect("partition recovery timeout checkpoint")
                    .map(|quorum| quorum.checkpoint.id().expect("trusted checkpoint id"));
                panic!(
                    "partitioned member did not become ready after ordered recovery; maximum synced peers: {maximum_synced}; required remote quorum: {required_remote_quorum}; readiness error: {err}; local_status={local_status:?}; live_session_active={}; replay_ready={}; trusted_checkpoint={trusted_checkpoint:?}",
                    state.federation.live_session_active().await,
                    state.federation.live_session_replay_ready().await,
                )
            }
        }
    }
}

const LATENCY_EXPECTED_STAGES: &[CdkAlephBftLatencyStage] = &[
    CdkAlephBftLatencyStage::SubmitQueued,
    CdkAlephBftLatencyStage::DataProviderYielded,
    CdkAlephBftLatencyStage::NetworkMessageDelivered,
    CdkAlephBftLatencyStage::FinalizedDataReceived,
    CdkAlephBftLatencyStage::ApplyLoopReceived,
    CdkAlephBftLatencyStage::ApplyCompleted,
    CdkAlephBftLatencyStage::OperationNotified,
    CdkAlephBftLatencyStage::WaiterReturned,
];

struct TwoMemberAlephBftLiveSession {
    first_state: MintState,
    second_state: MintState,
    _first_handle: Arc<AlephBftSessionSupervisor>,
    _second_handle: Arc<AlephBftSessionSupervisor>,
}

