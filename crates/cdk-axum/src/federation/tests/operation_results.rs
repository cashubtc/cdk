#[tokio::test]
async fn submit_mint_quote_operation_applies_quote_to_local_store() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint quote");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
    assert_eq!(quote.amount, Some(Amount::new(1, CurrencyUnit::Sat)));
    assert_eq!(quote.state(), MintQuoteState::Unpaid);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(
        state
            .federation
            .consensus
            .as_ref()
            .expect("consensus")
            .lock()
            .await
            .journal()
            .entry_by_operation_id(operation_id)
            .expect("runtime entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn submit_mint_quote_rejects_invalid_admission_before_journal_append() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = invalid_mint_quote_transcript_envelope(&state, &quote_id);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("invalid quote transcript should fail before consensus");

    assert!(
        matches!(err, cdk::Error::UnknownQuote),
        "unexpected invalid mint quote error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .is_none());
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
}

#[tokio::test]
async fn submit_mint_operation_rejects_invalid_admission_before_journal_append() {
    let (state, _, _) = federation_route_state().await;
    let envelope = invalid_mint_operation_keyset_envelope(&state);

    let err = state
        .submit_federation_envelope(envelope)
        .await
        .expect_err("invalid mint operation should fail before consensus");

    assert!(matches!(
        err,
        cdk::Error::Federation(FederationError::InvalidFederatedKeysetVersion { .. })
    ));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
}

#[tokio::test]
async fn submit_payment_observation_rejects_invalid_admission_without_dkg_sequence_mutation() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let mut envelope = paid_observation_envelope(&state, &quote_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("expected mint quote payment observation");
    };
    observation.method = PaymentMethod::Custom("bad/method".to_string());
    let envelope = envelope
        .authorize(
            state
                .mint
                .federation_config()
                .expect("federation config")
                .private_config(),
        )
        .expect("authorize invalid payment observation");
    let err = state
        .submit_federation_envelope_to_consensus(envelope)
        .await
        .expect_err("invalid payment observation should fail before consensus");

    assert!(matches!(err, cdk::Error::InvalidPaymentMethod));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert!(entries.is_empty());
    let sequences = state
        .mint
        .localstore()
        .get_federation_dkg_member_sequences()
        .await
        .expect("sequence lookup");
    assert!(
        sequences.is_empty(),
        "invalid operation admission must not mutate durable DKG replay state"
    );
}

#[tokio::test]
async fn local_status_uses_materialized_database_state_digest() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let status = local_peer_status(&state).await.expect("local status");
    assert_eq!(status.operation_count, 1);
    assert_ne!(
        status.materialized_state_digest,
        Some(FederationMaterializedStateDigest::genesis())
    );
}

#[tokio::test]
async fn materialized_state_digest_excludes_rejection_metadata() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let mut spent_entry = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup")
        .into_iter()
        .next()
        .expect("journal entry");
    spent_entry.application = FederationApplicationState::Rejected {
        response: cdk_common::error::ErrorResponse::from(cdk::Error::TokenAlreadySpent),
    };
    let mut duplicate_entry = spent_entry.clone();
    duplicate_entry.application = FederationApplicationState::Rejected {
        response: cdk_common::error::ErrorResponse::from(cdk::Error::DuplicateInputs),
    };

    assert_eq!(spent_entry.operation_id, duplicate_entry.operation_id);
    assert_eq!(spent_entry.order_digest, duplicate_entry.order_digest);

    let spent_digest = materialized_state_digest(&state, &[spent_entry])
        .await
        .expect("spent rejection digest");
    let duplicate_digest = materialized_state_digest(&state, &[duplicate_entry])
        .await
        .expect("duplicate rejection digest");

    assert_eq!(spent_digest, duplicate_digest);
}

#[tokio::test]
async fn materialized_state_digest_tracks_keyset_rotation_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("before digest");
    let keyset = public_config.keysets[0].clone();
    let envelope = state
        .mint
        .federation_config()
        .expect("federation config")
        .operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation { keyset },
        ));
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit keyset rotation");
    let after_entries = consensus.lock().await.journal().entries().to_vec();
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|entry| entry.operation_id == operation_id)
            .expect("rotation entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn materialized_state_digest_ignores_local_mint_payment_metadata() {
    let (first_state, _, _) = federation_route_state().await;
    let (second_state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
    let paid_envelope = paid_observation_envelope(&first_state, &quote_id);

    for state in [&first_state, &second_state] {
        state
            .submit_federation_envelope(quote_envelope.clone())
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope(paid_envelope.clone())
            .await
            .expect("submit paid observation");
    }

    let mut tx = second_state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin tx");
    let mut quote = tx
        .get_mint_quote(&quote_id)
        .await
        .expect("load quote")
        .expect("mint quote");
    quote
        .add_payment(
            Amount::from(0).with_unit(CurrencyUnit::Sat),
            "local-backend-zero-payment".to_string(),
            Some(42),
        )
        .expect("add local payment metadata");
    tx.update_mint_quote(&mut quote)
        .await
        .expect("persist local payment metadata");
    tx.commit().await.expect("commit tx");

    let first_entries = first_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();

    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second digest");

    assert_eq!(first_digest, second_digest);
}

#[tokio::test]
async fn materialized_state_digest_tracks_unfinalized_payment_observation_rows() {
    let (mut public_config, local_private, _) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state = federation_route_state_for_private(public_config, local_private).await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let before_entries = consensus.lock().await.journal().entries().to_vec();
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before observation");

    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit unfinalized observation");
    let after_entries = consensus.lock().await.journal().entries().to_vec();
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after observation");

    assert_ne!(
            before_digest, after_digest,
            "accepted payment-observation evidence must affect the materialized digest before quote finality"
        );

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state(), MintQuoteState::Unpaid);
    assert_eq!(quote.amount_paid(), Amount::new(0, CurrencyUnit::Sat));

    let observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            &quote_id.to_string(),
        )
        .await
        .expect("payment observations");
    assert_eq!(observations.len(), 1);
}

#[tokio::test]
async fn materialized_state_digest_tracks_swap_spend_and_output_share_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let input = proof_for_amount(&state, 1, "digest-swap");
    let input_y = input.y().expect("input y");
    let output = swap_output(&state, "digest-swap");
    let request = cdk::nuts::SwapRequest::new(vec![input], vec![output.clone()]);
    let envelope = config.swap_operation(request).expect("swap envelope");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted swap");
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&[input_y])
        .await
        .expect("proof state lookup");
    assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_some());
}

#[tokio::test]
async fn failed_mint_apply_preserves_materialized_digest_and_replays_on_restart() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let seed = [41_u8; 64];
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let mut consensus = test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let quote_id = QuoteId::new();

    let quote_entry = finalize_test_entry(&mut consensus, mint_quote_envelope(&state, &quote_id));
    persist_test_journal_entry(&state, &quote_entry).await;
    state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&quote_entry))
        .await
        .expect("apply mint quote");
    let payment_entry =
        finalize_test_entry(&mut consensus, paid_observation_envelope(&state, &quote_id));
    persist_test_journal_entry(&state, &payment_entry).await;
    state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&payment_entry))
        .await
        .expect("apply mint payment");

    let (mint_envelope, output) =
        mint_operation_envelope(&state, &quote_id, b"atomic mint apply output");
    let mint_entry = finalize_test_entry(&mut consensus, mint_envelope);
    add_blinded_message_conflict(&state, Some(&quote_id), &output).await;
    persist_test_journal_entry(&state, &mint_entry).await;
    let before_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal before failed mint apply");
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before failed mint apply");

    let err = state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
        .await
        .expect_err("conflicting blinded message must fail mint apply");
    assert!(matches!(
        err,
        cdk::Error::Database(cdk_common::database::Error::Duplicate)
    ));

    let after_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after failed mint apply");
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after failed mint apply");
    assert_eq!(after_digest, before_digest);
    assert_eq!(after_entries.len(), before_entries.len());
    let failed_entry = after_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("mint entry after failure");
    assert_eq!(failed_entry.status(), FederationOperationStatus::Accepted);
    let quote = db
        .get_mint_quote(&quote_id)
        .await
        .expect("quote after failed mint apply")
        .expect("stored mint quote");
    assert_eq!(quote.state(), MintQuoteState::Paid);
    assert_eq!(
        quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(db
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("mint shares after failed apply")
        .is_empty());
    assert!(db
        .get_completed_operation(&federation_completed_operation_uuid(
            mint_entry.operation_id,
        ))
        .await
        .expect("mint completed operation after failed apply")
        .is_none());
    assert_eq!(
        db.get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("mint signature after failed apply"),
        vec![None]
    );

    remove_blinded_message_conflict(&state, &output).await;
    drop(state);
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("restart replay after failed mint apply");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    let replayed_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after mint restart replay");
    assert_eq!(
        replayed_entries
            .iter()
            .find(|entry| entry.operation_id == mint_entry.operation_id)
            .expect("replayed mint entry")
            .status(),
        FederationOperationStatus::Applied
    );
    let replayed_digest = materialized_state_digest(&restarted_state, &replayed_entries)
        .await
        .expect("digest after mint restart replay");
    assert_ne!(replayed_digest, before_digest);
    assert_eq!(
        db.get_mint_quote(&quote_id)
            .await
            .expect("replayed mint quote")
            .expect("mint quote after replay")
            .state(),
        MintQuoteState::Issued
    );
    assert!(db
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("mint signature after replay")[0]
        .is_some());
}

#[tokio::test]
async fn failed_swap_apply_preserves_materialized_digest_and_replays_on_restart() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let seed = [42_u8; 64];
    let state = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    let mut consensus = test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let input = proof_for_amount(&state, 1, "atomic-swap-apply-input");
    let input_y = input.y().expect("swap input y");
    let output = swap_output(&state, "atomic-swap-apply-output");
    let config = state.mint.federation_config().expect("federation config");
    let envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input],
            vec![output.clone()],
        ))
        .expect("swap envelope");
    let swap_entry = finalize_test_entry(&mut consensus, envelope);
    add_blinded_message_conflict(&state, None, &output).await;
    persist_test_journal_entry(&state, &swap_entry).await;
    let before_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal before failed swap apply");
    let before_digest = materialized_state_digest(&state, &before_entries)
        .await
        .expect("digest before failed swap apply");

    let err = state
        .mint
        .apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
        .await
        .expect_err("conflicting blinded message must fail swap apply");
    assert!(matches!(
        err,
        cdk::Error::Database(cdk_common::database::Error::Duplicate)
    ));

    let after_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after failed swap apply");
    let after_digest = materialized_state_digest(&state, &after_entries)
        .await
        .expect("digest after failed swap apply");
    assert_eq!(after_digest, before_digest);
    assert_eq!(after_entries.len(), before_entries.len());
    assert_eq!(
        after_entries
            .iter()
            .find(|entry| entry.operation_id == swap_entry.operation_id)
            .expect("swap entry after failure")
            .status(),
        FederationOperationStatus::Accepted
    );
    assert_eq!(
        db.get_proofs_states(&[input_y])
            .await
            .expect("proof state after failed swap apply"),
        vec![None]
    );
    assert!(db
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("swap shares after failed apply")
        .is_empty());
    assert!(db
        .get_completed_operation(&federation_completed_operation_uuid(
            swap_entry.operation_id,
        ))
        .await
        .expect("swap completed operation after failed apply")
        .is_none());
    assert_eq!(
        db.get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("swap signature after failed apply"),
        vec![None]
    );

    remove_blinded_message_conflict(&state, &output).await;
    drop(state);
    let mut restarted_state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (replayed_consensus, replayed_sequences) =
        crate::create_federation_runtime_state(&restarted_state.mint)
            .await
            .expect("restart replay after failed swap apply");
    restarted_state.federation.consensus = replayed_consensus;
    restarted_state.federation.sequences = replayed_sequences;

    let replayed_entries = db
        .get_federation_journal_entries()
        .await
        .expect("journal after swap restart replay");
    assert_eq!(
        replayed_entries
            .iter()
            .find(|entry| entry.operation_id == swap_entry.operation_id)
            .expect("replayed swap entry")
            .status(),
        FederationOperationStatus::Applied
    );
    let replayed_digest = materialized_state_digest(&restarted_state, &replayed_entries)
        .await
        .expect("digest after swap restart replay");
    assert_ne!(replayed_digest, before_digest);
    assert_eq!(
        db.get_proofs_states(&[input_y])
            .await
            .expect("proof state after swap replay"),
        vec![Some(cdk::nuts::State::Spent)]
    );
    assert!(db
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("swap signature after replay")[0]
        .is_some());
}

#[tokio::test]
async fn submit_federation_envelope_notifies_operation_acceptance() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let waiter_state = state.clone();
    let waiter = tokio::spawn(async move {
        waiter_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified response")
    });
    tokio::task::yield_now().await;

    let response = state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint quote");
    let notified = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .expect("notification waiter")
        .expect("notification task");

    assert_eq!(response.operation_id, operation_id);
    assert_eq!(notified.operation_id, operation_id);
    assert_eq!(notified.outcome.entry().operation_id, operation_id);
}

#[tokio::test]
async fn submit_federation_envelope_returns_duplicate_when_acceptance_races_submission() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");

    state
        .validate_federation_envelope_admission(&envelope)
        .await
        .expect("operation admitted before race");

    let accepted = state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("operation accepted by competing path");
    assert_eq!(accepted.operation_id, operation_id);
    assert!(matches!(
        accepted.outcome,
        FederationJournalAppendOutcome::Appended(_)
    ));

    let duplicate = state
        .submit_federation_envelope(envelope)
        .await
        .expect("post-race submit should return duplicate response");
    assert_eq!(duplicate.operation_id, operation_id);
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));

    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Applied);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("mint quote lookup")
        .is_some());

    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.duplicate_submissions, 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
}

#[tokio::test]
async fn submit_federation_envelope_bounds_response_cache() {
    let (mut state, _, _) = federation_route_state().await;
    state.federation.peer_sync_config = Some(FederationPeerSyncConfig {
        response_cache_limit: 1,
        ..FederationPeerSyncConfig::default()
    });
    let first_quote_id = QuoteId::new();
    let first_envelope = mint_quote_envelope(&state, &first_quote_id);
    let first_operation_id = first_envelope.operation_id().expect("first operation id");
    let first_response = state
        .submit_federation_envelope(first_envelope.clone())
        .await
        .expect("submit first operation");
    assert_eq!(first_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let second_quote_id = QuoteId::new();
    let second_response = state
        .submit_federation_envelope(mint_quote_envelope(&state, &second_quote_id))
        .await
        .expect("submit second operation");
    assert_ne!(second_response.operation_id, first_operation_id);
    assert_eq!(state.federation_response_cache_entry_count(), 1);

    let duplicate_response = state
        .submit_federation_envelope(first_envelope)
        .await
        .expect("resubmit first operation after cache eviction");
    assert_eq!(duplicate_response.operation_id, first_operation_id);
    assert!(matches!(
        duplicate_response.outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(state.federation_response_cache_entry_count(), 1);
    let stats = state.federation_operation_notification_stats();
    assert_eq!(stats.response_cache_items, 1);
    assert_eq!(stats.response_cache_lookups, 2);
    assert_eq!(stats.response_cache_hits, 0);
    assert_eq!(stats.duplicate_submissions, 1);
    assert_eq!(stats.duplicate_pending_waiter_joins, 0);
}

#[tokio::test]
async fn catch_up_replay_materializes_completed_operation_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let output = mint_output(&state, &quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_envelope = config.mint_operation(mint_input).expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    finalize_test_operation(
        &mut peer_consensus,
        paid_observation_envelope(&state, &quote_id),
    )
    .expect("peer accepted paid observation");
    finalize_test_operation(&mut peer_consensus, mint_envelope).expect("peer accepted mint");
    let catch_up_request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_snapshot = materialized_state_snapshot(&state)
        .await
        .expect("before snapshot");
    assert!(
        !before_snapshot.rows().iter().any(|row| {
            row.table == "completed_operations"
                && row.key == federation_completed_operation_uuid(mint_operation_id).to_string()
        }),
        "a lagging member must not discover an unapplied completed-operation row from history"
    );

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 3);

    let snapshot = materialized_state_snapshot(&state)
        .await
        .expect("materialized snapshot");
    snapshot.digest().expect("materialized digest");
    let completed_row = completed_operation_snapshot_row(&snapshot, mint_operation_id);
    assert_eq!(
        completed_row.fields.get("kind").map(String::as_str),
        Some("mint")
    );
    assert!(!completed_row.fields.contains_key("missing"));
}

#[tokio::test]
async fn catch_up_replay_materializes_unresolved_payment_observation_rows() {
    let (mut public_config, local_private, peer_private) = transport_config();
    public_config.payment_observation_policy = FederationPaymentObservationPolicy::member_quorum(2);
    let state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let local_member = local_private.member_id;
    let peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = QuoteId::new();
    let observation =
        mint_payment_observation_envelope_for_member(&state, &quote_id, 1, 1, "settled");
    let mut peer_consensus =
        test_aleph_bft_peer(public_config.federation_id, peer_member.member_id);
    finalize_test_operation(&mut peer_consensus, mint_quote_envelope(&state, &quote_id))
        .expect("peer accepted mint quote");
    finalize_test_operation(&mut peer_consensus, observation)
        .expect("peer accepted unresolved payment observation");
    let catch_up_request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let applied = state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up response");
    assert_eq!(applied.len(), 2);

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state(), MintQuoteState::Unpaid);
    assert_eq!(quote.amount_paid(), Amount::new(0, CurrencyUnit::Sat));
    let observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            &quote_id.to_string(),
        )
        .await
        .expect("payment observations");
    assert_eq!(observations.len(), 1);

    let status_request = signed_status_request(&public_config, &peer_private, local_member, 1);
    let status = post_local_status(State(state), Json(status_request))
        .await
        .expect("local status response")
        .0;
    assert_eq!(status.payment_observation_reports.len(), 1);
    let report = status
        .payment_observation_reports
        .first()
        .expect("payment observation report");
    assert_eq!(
        report.status,
        cdk::federation::FederationPaymentObservationReportStatus::AwaitingQuorum
    );
    assert_eq!(report.required, 2);
    assert_eq!(report.total_observations, 1);
    assert!(!report.groups[0].quorum_reached);
    assert!(report.stale);
    assert_eq!(
        report.recommended_action,
        FederationPaymentObservationReportAction::Inspect
    );
    let status_json = serde_json::to_string(&status).expect("status json");
    assert!(!status_json.contains("settled"));
}

#[cfg(feature = "prometheus")]
#[tokio::test]
async fn submit_federation_envelope_records_prometheus_operation_metrics() {
    let (state, _, _) = federation_route_state().await;
    let member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id()
        .to_string();
    let accepted_labels = [("member", member.as_str()), ("operation", "mint_quote")];
    let rejected_labels = [
        ("member", member.as_str()),
        ("operation", "mint_quote"),
        ("reason", "consensus"),
    ];

    let accepted_before =
        prometheus_metric_value("cdk_federation_accepted_operations_total", &accepted_labels);
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    let accepted_after =
        prometheus_metric_value("cdk_federation_accepted_operations_total", &accepted_labels);
    assert_eq!(accepted_after, accepted_before + 1.0);

    let rejected_before =
        prometheus_metric_value("cdk_federation_rejected_operations_total", &rejected_labels);
    let mut conflicting = mint_quote_envelope(&state, &quote_id);
    let FederationOperation::MintQuote(operation) = &mut conflicting.operation else {
        panic!("mint quote operation");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("bolt11 mint quote response");
    };
    response.request.push_str("-conflict");
    let conflicting = conflicting
        .authorize(
            state
                .mint
                .federation_config()
                .expect("federation config")
                .private_config(),
        )
        .expect("reauthorize conflicting mint quote fixture");

    state
        .submit_federation_envelope(conflicting)
        .await
        .expect_err("conflicting mint quote must be rejected");
    let rejected_after =
        prometheus_metric_value("cdk_federation_rejected_operations_total", &rejected_labels);
    assert_eq!(rejected_after, rejected_before + 1.0);
}

#[tokio::test]
async fn submit_mint_operation_applies_issuance_and_persists_share() {
    let (state, _, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &quote_id))
        .await
        .expect("submit paid observation");

    let output = mint_output(&state, &quote_id);
    let input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let envelope = config.mint_operation(input.clone()).expect("mint envelope");
    let operation_id = envelope.operation_id().expect("operation id");
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit mint");

    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.amount_paid(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));
    assert_eq!(quote.state(), MintQuoteState::Issued);

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup");
    let persisted_signature = signatures[0].clone().expect("signature persisted");
    let expected_response = config
        .mint_share_response(operation_id, &input)
        .expect("expected share response");
    assert_eq!(persisted_signature, expected_response.signatures[0]);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(
        persisted
            .iter()
            .find(|entry| entry.operation_id == operation_id)
            .expect("mint entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn materialized_state_digest_ignores_member_local_signature_share_bytes() {
    let (public_config, first_private, second_private) = transport_config();
    let first_state =
        federation_route_state_for_private(public_config.clone(), first_private).await;
    let second_state =
        federation_route_state_for_private(public_config.clone(), second_private).await;
    let quote_id = QuoteId::new();
    let quote_envelope = mint_quote_envelope(&first_state, &quote_id);
    let paid_envelope = paid_observation_envelope(&first_state, &quote_id);
    let output = mint_output(&first_state, &quote_id);
    let input = MintInput::Single(MintRequest {
        quote: quote_id.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_envelope = first_state
        .mint
        .federation_config()
        .expect("federation config")
        .mint_operation(input)
        .expect("mint envelope");

    for state in [&first_state, &second_state] {
        state
            .submit_federation_envelope_to_consensus(quote_envelope.clone())
            .await
            .expect("submit mint quote");
        state
            .submit_federation_envelope_to_consensus(paid_envelope.clone())
            .await
            .expect("submit paid observation");
        state
            .submit_federation_envelope_to_consensus(mint_envelope.clone())
            .await
            .expect("submit mint");
    }

    let first_signature = first_state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("first signature lookup")
        .remove(0)
        .expect("first signature");
    let second_signature = second_state
        .mint
        .localstore()
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("second signature lookup")
        .remove(0)
        .expect("second signature");
    assert_ne!(first_signature.c, second_signature.c);

    let first_entries = first_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second digest");

    assert_eq!(first_digest, second_digest);
}

#[tokio::test]
async fn materialized_state_digest_tracks_melt_quote_spend_and_change_rows() {
    let (state, public_config, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let _peer_member = public_config.member(member_id(2)).expect("peer member");
    let quote_id = melt_quote(&state, 1000).await;
    let inputs = vec![
        proof_for_amount(&state, 1, "digest-melt-a"),
        proof_for_amount(&state, 1, "digest-melt-b"),
        proof_for_amount(&state, 1, "digest-melt-c"),
    ];
    let input_ys = inputs
        .iter()
        .map(|proof| proof.y().expect("input y"))
        .collect::<Vec<_>>();
    let mut change_output = swap_output(&state, "digest-melt-change");
    change_output.amount = Amount::ZERO;
    let request =
        cdk::nuts::MeltRequest::new(quote_id.clone(), inputs, Some(vec![change_output.clone()]));
    let envelope = config.melt_operation(request).expect("melt envelope");
    let payment_envelope = melt_payment_observation_envelope(
        &state,
        &quote_id,
        MeltQuoteState::Paid,
        Some(String::new()),
    );
    let local_consensus = state.federation.consensus.as_ref().expect("consensus");
    let mut peer_consensus = local_consensus.lock().await.clone();
    finalize_test_operation(&mut peer_consensus, envelope).expect("peer accepted melt");
    finalize_test_operation(&mut peer_consensus, payment_envelope)
        .expect("peer accepted melt payment observation");
    let catch_up_request = local_consensus.lock().await.catch_up_request();
    let catch_up_response = checked_test_catch_up_response(&peer_consensus, &catch_up_request)
        .expect("catch-up response");

    let before_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("before digest");
    state
        .apply_federation_catch_up_response(&catch_up_response)
        .await
        .expect("apply catch-up");
    let after_digest = materialized_state_digest(&state, peer_consensus.journal().entries())
        .await
        .expect("after digest");

    assert_ne!(before_digest, after_digest);
    assert_ne!(after_digest, FederationMaterializedStateDigest::genesis());

    let quote = state
        .mint
        .localstore()
        .get_melt_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("quote persisted");
    assert_eq!(quote.state, MeltQuoteState::Paid);

    let proof_states = state
        .mint
        .localstore()
        .get_proofs_states(&input_ys)
        .await
        .expect("proof state lookup");
    assert_eq!(
        proof_states,
        vec![
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
            Some(cdk::nuts::State::Spent),
        ]
    );

    let signatures = state
        .mint
        .localstore()
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("signature lookup");
    assert!(signatures[0].is_some());
}

fn completed_operation_snapshot_row(
    snapshot: &FederationMaterializedStateSnapshot,
    operation_id: FederationOperationId,
) -> &FederationMaterializedStateRow {
    let operation_uuid = federation_completed_operation_uuid(operation_id);
    snapshot
        .rows()
        .iter()
        .find(|row| row.table == "completed_operations" && row.key == operation_uuid.to_string())
        .expect("completed operation materialized row")
}

#[tokio::test]
async fn materialized_state_digest_tracks_completed_operation_rows_for_value_operations() {
    let (state, _, _) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();

    let mint_quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &mint_quote_id))
        .await
        .expect("submit mint quote");
    state
        .submit_federation_envelope(paid_observation_envelope(&state, &mint_quote_id))
        .await
        .expect("submit paid observation");
    let mint_output = mint_output(&state, &mint_quote_id);
    let mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id,
        outputs: vec![mint_output],
        signature: None,
    });
    let mint_envelope = config.mint_operation(mint_input).expect("mint envelope");
    let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
    state
        .submit_federation_envelope(mint_envelope)
        .await
        .expect("submit mint operation");

    let swap_input = proof_for_amount(&state, 1, "completed-operation-swap");
    let swap_change = swap_output(&state, "completed-operation-swap");
    let swap_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![swap_input],
            vec![swap_change],
        ))
        .expect("swap envelope");
    let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");
    state
        .submit_federation_envelope(swap_envelope)
        .await
        .expect("submit swap operation");

    let paid_melt_description = FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Paid,
        check_payment_state: MeltQuoteState::Paid,
        pay_err: false,
        check_err: false,
    };
    let melt_quote_id = melt_quote_with_description(
        &state,
        1_000,
        serde_json::to_string(&paid_melt_description).expect("paid melt description"),
    )
    .await;
    let melt_inputs = vec![
        proof_for_amount(&state, 1, "completed-operation-melt-a"),
        proof_for_amount(&state, 1, "completed-operation-melt-b"),
        proof_for_amount(&state, 1, "completed-operation-melt-c"),
    ];
    let mut melt_change = swap_output(&state, "completed-operation-melt-change");
    melt_change.amount = Amount::ZERO;
    let melt_envelope = config
        .melt_operation(cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            melt_inputs,
            Some(vec![melt_change]),
        ))
        .expect("melt envelope");
    let melt_operation_id = melt_envelope.operation_id().expect("melt operation id");
    state
        .submit_federation_envelope(melt_envelope)
        .await
        .expect("submit melt operation");
    state
        .submit_federation_envelope(melt_payment_observation_envelope(
            &state,
            &melt_quote_id,
            MeltQuoteState::Paid,
            Some(String::new()),
        ))
        .await
        .expect("submit melt payment observation");

    let snapshot = materialized_state_snapshot(&state)
        .await
        .expect("materialized snapshot");
    let digest = snapshot.digest().expect("materialized digest");
    assert_ne!(digest, FederationMaterializedStateDigest::genesis());

    let mint_row = completed_operation_snapshot_row(&snapshot, mint_operation_id);
    assert_eq!(
        mint_row.fields.get("kind").map(String::as_str),
        Some("mint")
    );
    assert_eq!(
        mint_row.fields.get("total_issued").map(String::as_str),
        Some("1")
    );
    assert!(!mint_row.fields.contains_key("missing"));

    let swap_row = completed_operation_snapshot_row(&snapshot, swap_operation_id);
    assert_eq!(
        swap_row.fields.get("kind").map(String::as_str),
        Some("swap")
    );
    assert_eq!(
        swap_row.fields.get("total_issued").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        swap_row.fields.get("total_redeemed").map(String::as_str),
        Some("1")
    );
    assert!(!swap_row.fields.contains_key("missing"));

    let melt_row = completed_operation_snapshot_row(&snapshot, melt_operation_id);
    assert_eq!(
        melt_row.fields.get("kind").map(String::as_str),
        Some("melt")
    );
    assert_eq!(
        melt_row.fields.get("total_redeemed").map(String::as_str),
        Some("3")
    );
    assert!(!melt_row.fields.contains_key("missing"));

    let fingerprints = snapshot
        .row_fingerprints()
        .expect("materialized row fingerprints");
    assert!(
        fingerprints
            .iter()
            .filter(|row| row.table == "completed_operations")
            .count()
            >= 3
    );
    let fingerprints_json =
        serde_json::to_string(&fingerprints).expect("materialized fingerprint json");
    assert!(!fingerprints_json.contains(&mint_operation_id.to_string()));
    assert!(!fingerprints_json.contains(&swap_operation_id.to_string()));
    assert!(!fingerprints_json.contains(&melt_operation_id.to_string()));
}

#[tokio::test]
#[ignore = "manual materialized-state projection benchmark"]
async fn benchmark_materialized_state_snapshot_growth() {
    for operation_count in [100_usize, 500, 1_000] {
        let (state, _, _) = federation_route_state().await;
        let mut journal = InMemoryFederationJournal::new();
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin benchmark transaction");

        for _ in 0..operation_count {
            let quote_id = QuoteId::new();
            let quote = MintQuote::new(
                Some(quote_id.clone()),
                format!("invoice-{quote_id}"),
                CurrencyUnit::Sat,
                Some(Amount::from(1).with_unit(CurrencyUnit::Sat)),
                10_000,
                PaymentIdentifier::QuoteId(quote_id.clone()),
                None,
                Amount::ZERO.with_unit(CurrencyUnit::Sat),
                Amount::ZERO.with_unit(CurrencyUnit::Sat),
                PaymentMethod::Known(KnownMethod::Bolt11),
                1,
                1,
                Vec::new(),
                Vec::new(),
                None,
            );
            tx.add_mint_quote(quote).await.expect("add benchmark quote");
            journal
                .append(mint_quote_envelope(&state, &quote_id))
                .expect("append benchmark operation");
        }
        tx.commit().await.expect("commit benchmark quotes");

        let started = std::time::Instant::now();
        let snapshot = materialized_state_snapshot(&state)
            .await
            .expect("benchmark snapshot");
        let digest = snapshot.digest().expect("benchmark digest");
        let elapsed = started.elapsed();
        assert_eq!(snapshot.rows().len(), operation_count);
        assert_ne!(digest, FederationMaterializedStateDigest::genesis());
        eprintln!(
            "materialized_state_snapshot operations={operation_count} elapsed_us={} rows={}",
            elapsed.as_micros(),
            snapshot.rows().len()
        );
    }
}

#[cfg(feature = "prometheus")]
fn synthetic_conflicting_mint_quote_envelope(
    base: &FederationOperationEnvelope,
    private_config: &MemberConfigPrivate,
    operation_index: u64,
) -> FederationOperationEnvelope {
    let mut envelope = base.clone();
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("synthetic history base must be a mint quote");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("synthetic history base must use a bolt11 mint quote");
    };
    response
        .request
        .push_str(&format!("-synthetic-conflict-{operation_index}"));
    envelope
        .authorize(private_config)
        .expect("authorize synthetic conflicting operation")
}

#[cfg(feature = "prometheus")]
async fn reset_synthetic_history_consensus(
    state: &mut MintState,
    public_config: &FederationConfig,
    base_entry: &FederationJournalEntry,
) {
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("synthetic history checkpoint frontier");
    let (status, next_unit_index_by_member) = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("synthetic history consensus")
            .lock()
            .await;
        (
            consensus.status(),
            consensus.next_unit_index_by_member().clone(),
        )
    };
    let boundary = FederationCheckpointBoundary::from_complete_unit(
        &frontier,
        status.session,
        status.next_unit_finalization_index,
        next_unit_index_by_member,
    )
    .expect("synthetic history complete-unit boundary");
    let checkpoint = boundary
        .recovery_checkpoint(
            public_config,
            None,
            FederationMaterializedStateDigest::genesis(),
        )
        .expect("synthetic history transient checkpoint");
    let mut consensus = FederationConsensusRuntime::checkpoint_base_for_test(
        public_config,
        status.local_member,
        &checkpoint,
    )
    .expect("reset synthetic history consensus");
    consensus
        .register_historical_journal_entry(base_entry.clone())
        .expect("register synthetic history conflict anchor");
    state.federation.consensus = Some(Arc::new(tokio::sync::Mutex::new(consensus)));
}

#[cfg(feature = "prometheus")]
async fn append_synthetic_history_conflicts(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    base_envelope: &FederationOperationEnvelope,
    base_entry: &FederationJournalEntry,
    mut next_operation_index: u64,
    operation_count: u64,
) {
    while next_operation_index < operation_count {
        let batch_len = usize::try_from(
            operation_count
                .saturating_sub(next_operation_index)
                .min(cdk::federation::FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS as u64),
        )
        .expect("synthetic batch length fits in usize");
        let items = (0..batch_len)
            .map(|offset| {
                let operation_index = next_operation_index.saturating_add(
                    u64::try_from(offset).expect("synthetic batch offset fits in u64"),
                );
                FederationConsensusItem::operation(synthetic_conflicting_mint_quote_envelope(
                    base_envelope,
                    local_private,
                    operation_index,
                ))
            })
            .collect::<Vec<_>>();
        let (finalized_items, entries) = {
            let mut consensus = state
                .federation
                .consensus
                .as_ref()
                .expect("synthetic history consensus")
                .lock()
                .await;
            let status = consensus.status();
            let payload = FederationAlephBftUnitPayload::new(
                status.federation_id,
                status.local_member,
                status.session,
                consensus.next_unit_index(),
                items,
            )
            .expect("synthetic history finalized payload");
            let finalized_start = consensus.finalized_items().len();
            let responses = consensus
                .finalize_aleph_bft_unit(payload)
                .expect("finalize synthetic history batch");
            let finalized_items = consensus.finalized_items()[finalized_start..].to_vec();
            let entries = responses
                .into_iter()
                .map(|response| match response.outcome {
                    FederationJournalAppendOutcome::Appended(entry) => entry,
                    FederationJournalAppendOutcome::Duplicate(_) => {
                        panic!("synthetic history operation must be unique")
                    }
                })
                .collect::<Vec<_>>();
            (finalized_items, entries)
        };
        assert_eq!(finalized_items.len(), batch_len);
        assert_eq!(entries.len(), batch_len);
        assert!(entries.iter().all(|entry| {
            matches!(
                entry.application,
                FederationApplicationState::ConsensusRejected {}
            )
        }));
        state
            .federation
            .persist_finalization(&finalized_items, &entries)
            .await
            .expect("persist synthetic history batch");
        next_operation_index = next_operation_index
            .saturating_add(u64::try_from(batch_len).expect("synthetic batch length fits in u64"));
        reset_synthetic_history_consensus(state, public_config, base_entry).await;
    }
}

#[cfg(feature = "prometheus")]
async fn build_synthetic_checkpoint_history(
    state: &mut MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    peer_private: &MemberConfigPrivate,
    operation_count: u64,
) -> FederationCheckpoint {
    assert!(operation_count > 0, "synthetic history must be non-empty");
    let quote_id = QuoteId::new();
    let base_envelope = mint_quote_envelope(state, &quote_id);
    state
        .submit_federation_envelope(base_envelope.clone())
        .await
        .expect("apply synthetic history conflict anchor");
    let base_entry = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(
            base_envelope.operation_id().expect("base operation id"),
        )
        .await
        .expect("load synthetic history conflict anchor")
        .expect("synthetic history conflict anchor exists");

    append_synthetic_history_conflicts(
        state,
        public_config,
        local_private,
        &base_envelope,
        &base_entry,
        1,
        operation_count,
    )
    .await;

    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("completed synthetic history frontier");
    assert_eq!(frontier.journal_next_index, operation_count);
    assert_eq!(frontier.application_next_index, operation_count);
    assert_eq!(frontier.finalized_next_item_index, operation_count);

    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        state,
        public_config,
        None,
        &[local_private, peer_private],
    )
    .await;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(public_config)
            .expect("verify synthetic checkpoint quorum"),
    )
    .await
    .expect("promote synthetic checkpoint");
    tx.commit()
        .await
        .expect("commit synthetic checkpoint promotion");
    checkpoint
}

#[cfg(feature = "prometheus")]
fn checkpoint_work_delta(member: &str, work: &str, before: f64) -> f64 {
    prometheus_metric_value(
        "cdk_federation_checkpoint_work_total",
        &[("member", member), ("work", work)],
    ) - before
}

#[cfg(feature = "prometheus")]
const SYNTHETIC_STARTUP_WORK_NAMES: [&str; 8] = [
    "journal_rows_loaded",
    "finalized_items_loaded",
    "signatures_verified",
    "startup_entries_replayed",
    "startup_tail_journal_rows_loaded",
    "startup_tail_finalized_items_loaded",
    "backup_bytes_loaded",
    "backup_chunks_loaded",
];

#[cfg(feature = "prometheus")]
struct SyntheticCheckpointStartupMeasurement {
    consensus: Arc<tokio::sync::Mutex<FederationConsensusRuntime>>,
    elapsed: Duration,
    work: [f64; SYNTHETIC_STARTUP_WORK_NAMES.len()],
    materialized_state_audits: f64,
}

#[cfg(feature = "prometheus")]
async fn measure_synthetic_checkpoint_startup(
    state: &MintState,
    public_config: &FederationConfig,
    local_private: &MemberConfigPrivate,
    load_active_session_backup: bool,
) -> SyntheticCheckpointStartupMeasurement {
    let member_label = local_private.member_id.to_string();
    let before = SYNTHETIC_STARTUP_WORK_NAMES.map(|work| {
        prometheus_metric_value(
            "cdk_federation_checkpoint_work_total",
            &[("member", member_label.as_str()), ("work", work)],
        )
    });
    let audit_labels = [
        ("member", member_label.as_str()),
        ("operation", "materialized_state_digest"),
    ];
    let audits_before = prometheus_metric_value(
        "cdk_federation_checkpoint_duration_seconds_count",
        &audit_labels,
    );

    let started = std::time::Instant::now();
    let (consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("restart from synthetic checkpoint");
    let consensus = consensus.expect("synthetic checkpoint consensus");
    if load_active_session_backup {
        let session = consensus.lock().await.status().session;
        let keychain = CdkAlephBftKeychain::try_new(public_config, local_private.clone())
            .expect("synthetic startup keychain");
        let db = state.mint.localstore();
        let _session_io = CdkAlephBftSessionIo::from_db(
            Arc::clone(&consensus),
            Arc::clone(&db),
            session,
            BTreeMap::new(),
            CdkAlephBftActivityHint::default(),
            keychain,
        )
        .await
        .expect("load synthetic active-session backup");
    }
    let elapsed = started.elapsed();
    let work = std::array::from_fn(|index| {
        checkpoint_work_delta(
            &member_label,
            SYNTHETIC_STARTUP_WORK_NAMES[index],
            before[index],
        )
    });
    let materialized_state_audits = prometheus_metric_value(
        "cdk_federation_checkpoint_duration_seconds_count",
        &audit_labels,
    ) - audits_before;

    SyntheticCheckpointStartupMeasurement {
        consensus,
        elapsed,
        work,
        materialized_state_audits,
    }
}

#[cfg(feature = "prometheus")]
async fn seed_synthetic_active_session_backup(
    state: &MintState,
    session: u64,
    chunk_count: usize,
    bytes_per_chunk: usize,
) {
    let chunks = (0..chunk_count)
        .map(|index| {
            vec![u8::try_from(index.saturating_add(1)).unwrap_or(u8::MAX); bytes_per_chunk]
        })
        .collect::<Vec<_>>();
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin synthetic backup transaction");
    tx.replace_federation_aleph_bft_backup_chunks(session, &chunks)
        .await
        .expect("persist synthetic backup chunks");
    tx.set_federation_aleph_bft_active_session(session)
        .await
        .expect("persist synthetic active session");
    tx.commit().await.expect("commit synthetic backup");
}

#[cfg(feature = "prometheus")]
fn print_synthetic_checkpoint_startup(
    scenario: &str,
    operation_count: u64,
    suffix_operations: u64,
    measurement: &SyntheticCheckpointStartupMeasurement,
) {
    eprintln!(
        "checkpoint_startup scenario={scenario} operations={operation_count} suffix_operations={suffix_operations} elapsed_us={} journal_rows_loaded={:.0} finalized_items_loaded={:.0} signatures_verified={:.0} startup_entries_replayed={:.0} startup_tail_journal_rows_loaded={:.0} startup_tail_finalized_items_loaded={:.0} backup_bytes_loaded={:.0} backup_chunks_loaded={:.0} materialized_state_audits={:.0}",
        measurement.elapsed.as_micros(),
        measurement.work[0],
        measurement.work[1],
        measurement.work[2],
        measurement.work[3],
        measurement.work[4],
        measurement.work[5],
        measurement.work[6],
        measurement.work[7],
        measurement.materialized_state_audits,
    );
}

#[cfg(feature = "prometheus")]
#[tokio::test]
#[ignore = "manual release-mode synthetic checkpoint startup benchmark"]
async fn benchmark_checkpoint_startup_is_history_length_independent() {
    for (operation_count, local_member_value, peer_member_value) in [
        (1_000_u64, 62_001_u16, 62_002_u16),
        (10_000_u64, 62_003_u16, 62_004_u16),
        (100_000_u64, 62_005_u16, 62_006_u16),
    ] {
        let local_member = member_id(local_member_value);
        let peer_member = member_id(peer_member_value);
        let (public_config, local_private, peer_private) =
            transport_config_with_members_and_federation_api_urls(
                local_member,
                peer_member,
                MintUrl::from_str(&format!(
                    "http://synthetic-checkpoint-{local_member_value}.test"
                ))
                .expect("synthetic local federation api url"),
                MintUrl::from_str(&format!(
                    "http://synthetic-checkpoint-{peer_member_value}.test"
                ))
                .expect("synthetic peer federation api url"),
            );
        let db_file = TempMintDbFile::new("cdk-axum-synthetic-checkpoint-startup");
        let mnemonic = Mnemonic::generate(12).expect("synthetic history mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let checkpoint = {
            let db = Arc::new(
                cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                    .await
                    .expect("synthetic history sqlite database"),
            );
            let mut state = federation_route_state_for_private_with_db(
                public_config.clone(),
                local_private.clone(),
                db,
                &seed,
            )
            .await;
            build_synthetic_checkpoint_history(
                &mut state,
                &public_config,
                &local_private,
                &peer_private,
                operation_count,
            )
            .await
        };
        assert_eq!(checkpoint.journal_next_index, operation_count);
        assert_eq!(checkpoint.finalized_next_item_index, operation_count);

        let reopened_db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("reopen synthetic history sqlite database"),
        );
        let mut restarted = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private.clone(),
            reopened_db,
            &seed,
        )
        .await;
        let exact =
            measure_synthetic_checkpoint_startup(&restarted, &public_config, &local_private, false)
                .await;
        {
            let consensus = exact.consensus.lock().await;
            assert_eq!(consensus.status().accepted_operation_count, operation_count);
            assert_eq!(consensus.status().finalized_item_count, operation_count);
            assert!(consensus.journal().entries().is_empty());
            assert!(consensus.finalized_items().is_empty());
        }
        assert_eq!(exact.work, [0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(exact.materialized_state_audits, 0.0);
        print_synthetic_checkpoint_startup("exact", operation_count, 0, &exact);

        restarted.federation.consensus = Some(exact.consensus);
        seed_synthetic_active_session_backup(&restarted, 0, 4, 1_024).await;
        drop(restarted);

        let backup_db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("reopen synthetic backup database"),
        );
        let mut backup_state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private.clone(),
            backup_db,
            &seed,
        )
        .await;
        let exact_with_backup = measure_synthetic_checkpoint_startup(
            &backup_state,
            &public_config,
            &local_private,
            true,
        )
        .await;
        {
            let consensus = exact_with_backup.consensus.lock().await;
            assert_eq!(consensus.status().accepted_operation_count, operation_count);
            assert_eq!(consensus.status().finalized_item_count, operation_count);
            assert!(consensus.journal().entries().is_empty());
            assert!(consensus.finalized_items().is_empty());
        }
        assert_eq!(
            exact_with_backup.work,
            [0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 4_096.0, 4.0]
        );
        assert_eq!(exact_with_backup.materialized_state_audits, 0.0);
        print_synthetic_checkpoint_startup(
            "exact_with_backup",
            operation_count,
            0,
            &exact_with_backup,
        );

        backup_state.federation.consensus = Some(exact_with_backup.consensus);
        let base_entry = backup_state
            .mint
            .localstore()
            .get_federation_journal_entries_range(0, 1, 1)
            .await
            .expect("load synthetic suffix conflict anchor")
            .pop()
            .expect("synthetic suffix conflict anchor exists");
        reset_synthetic_history_consensus(&mut backup_state, &public_config, &base_entry).await;
        let suffix_operations = 64_u64;
        let suffix_tip = operation_count
            .checked_add(suffix_operations)
            .expect("synthetic suffix tip");
        append_synthetic_history_conflicts(
            &mut backup_state,
            &public_config,
            &local_private,
            &base_entry.envelope,
            &base_entry,
            operation_count,
            suffix_tip,
        )
        .await;
        drop(backup_state);

        let tail_db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("reopen synthetic checkpoint tail database"),
        );
        let tail_state = federation_route_state_for_private_with_db(
            public_config.clone(),
            local_private.clone(),
            tail_db,
            &seed,
        )
        .await;
        let tail_with_backup =
            measure_synthetic_checkpoint_startup(&tail_state, &public_config, &local_private, true)
                .await;
        {
            let consensus = tail_with_backup.consensus.lock().await;
            assert_eq!(consensus.status().accepted_operation_count, suffix_tip);
            assert_eq!(consensus.status().finalized_item_count, suffix_tip);
            assert_eq!(consensus.journal().entries().len(), 64);
            assert_eq!(consensus.finalized_items().len(), 64);
        }
        assert_eq!(
            tail_with_backup.work,
            [64.0, 64.0, 2.0, 0.0, 64.0, 64.0, 4_096.0, 4.0]
        );
        assert_eq!(tail_with_backup.materialized_state_audits, 0.0);
        print_synthetic_checkpoint_startup(
            "fixed_tail_with_backup",
            operation_count,
            suffix_operations,
            &tail_with_backup,
        );
    }
}

