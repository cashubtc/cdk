use super::*;

#[tokio::test]
async fn federation_runtime_startup_rejects_aleph_bft_journal_mismatch() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let finalized_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mismatched_envelope = mint_quote_envelope(&state, &QuoteId::new());
    let mut adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("AlephBFT adapter");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(finalized_envelope)],
    )
    .expect("AlephBFT finalized unit");
    adapter.finalize_unit(unit).expect("finalize unit");
    let mut mismatched_consensus =
        test_aleph_bft_peer(public_config.federation_id, local_private.member_id);
    let mismatched_entry = finalize_test_operation(&mut mismatched_consensus, mismatched_envelope)
        .expect("mismatched journal entry")
        .outcome
        .entry()
        .clone();

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT mismatch transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&mismatched_entry)
        .await
        .expect("persist mismatched journal entry");
    tx.commit().await.expect("commit AlephBFT mismatch state");

    let err = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect_err("AlephBFT journal mismatch rejected");
    assert!(matches!(
        err.downcast_ref::<FederationError>(),
        Some(
            FederationError::AlephBftFinalizedItemRecoveryMismatch { .. }
                | FederationError::CatchUpOperationIdMismatch { .. }
        )
    ));
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_persists_applies_and_notifies() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(envelope)
            .expect("queue AlephBFT envelope");
        consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit")
    };

    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified operation")
    });
    tokio::task::yield_now().await;

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized AlephBFT unit");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].operation_id, operation_id);
    let notified = waiter.await.expect("waiter task");
    assert_eq!(notified.operation_id, operation_id);

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after finalization")
        .expect("mint quote applied");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 1);
    assert_eq!(stored_finalized_items[0].operation_id, Some(operation_id));
    let stored_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("stored journal entries");
    assert_eq!(stored_entries.len(), 1);
    assert_eq!(stored_entries[0].operation_id, operation_id);
    assert_eq!(
        stored_entries[0].status(),
        FederationOperationStatus::Applied
    );

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn federation_aleph_bft_duplicate_finalized_unit_applies_unmaterialized_entries() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let (unit, finalized_items, entries) = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        next_consensus
            .submit_aleph_bft_envelope(envelope)
            .expect("queue AlephBFT envelope");
        let unit = next_consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit");
        let finalized_item_start = next_consensus
            .aleph_bft_finalized_items()
            .expect("finalized items before replay")
            .len();
        let responses = next_consensus
            .finalize_aleph_bft_unit(unit.clone())
            .expect("finalize AlephBFT unit");
        let finalized_items = next_consensus
            .aleph_bft_finalized_items()
            .expect("finalized items after replay")[finalized_item_start..]
            .to_vec();
        let entries = responses
            .iter()
            .filter_map(|response| match &response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry.clone()),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        *consensus = next_consensus;
        (unit, finalized_items, entries)
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Accepted);

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin partial finalized-unit transaction");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist finalized items");
    for entry in &entries {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist accepted journal entry");
    }
    tx.commit()
        .await
        .expect("commit partial finalized-unit transaction");
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before duplicate replay")
        .is_none());

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("duplicate finalized unit replays accepted materialization");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].operation_id, operation_id);
    assert_eq!(
        responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after duplicate replay")
        .expect("quote materialized");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
    let entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after duplicate replay");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].operation_id, operation_id);
    assert_eq!(entries[0].status(), FederationOperationStatus::Applied);
}

#[tokio::test]
async fn duplicate_finalized_unit_recovers_lost_applied_response_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let unit = {
        let consensus = state.federation.consensus.as_ref().expect("consensus");
        let mut consensus = consensus.lock().await;
        consensus
            .submit_aleph_bft_envelope(envelope)
            .expect("queue AlephBFT envelope");
        consensus
            .next_aleph_bft_unit_payload()
            .expect("next AlephBFT unit")
            .expect("queued unit")
    };
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(operation_id)
            .await
            .expect("notified applied operation")
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    state
        .federation
        .suppress_next_terminal_response_publication_for_test();

    let first_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("durably apply finalized unit before publication interruption");
    assert_eq!(first_responses.len(), 1);
    assert_eq!(
        first_responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert!(!waiter.is_finished());
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after interrupted publication")
        .is_some());
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .update_application_state(operation_id, FederationApplicationState::Accepted {})
        .expect("simulate interruption before in-memory terminal overlay");

    let duplicate_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("duplicate finalized unit republishes terminal response");
    assert_eq!(duplicate_responses.len(), 1);
    assert!(matches!(
        &duplicate_responses[0].outcome,
        FederationJournalAppendOutcome::Duplicate(_)
    ));
    assert_eq!(
        duplicate_responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let notified = time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("applied waiter notification timeout")
        .expect("applied waiter task");
    assert_eq!(notified.operation_id, operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
        FederationOperationStatus::Applied
    );

    for _ in 0..4 {
        state
            .apply_federation_aleph_bft_finalized_unit(unit.clone())
            .await
            .expect("idempotent duplicate terminal publication");
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 1);
}

#[tokio::test]
async fn duplicate_finalized_unit_recovers_lost_rejected_response_publication() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let input = proof_for_amount(&state, 1, "lost-rejected-publication");
    let accepted_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input.clone()],
            vec![swap_output(&state, "lost-rejected-winner")],
        ))
        .expect("accepted swap envelope");
    let rejected_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input],
            vec![swap_output(&state, "lost-rejected-loser")],
        ))
        .expect("rejected swap envelope");
    let rejected_operation_id = rejected_envelope
        .operation_id()
        .expect("rejected operation id");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(accepted_envelope),
            FederationConsensusItem::operation(rejected_envelope),
        ],
    )
    .expect("conflicting finalized unit");
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(rejected_operation_id)
            .await
            .expect("notified rejected operation")
    });
    state
        .wait_for_federation_pending_response_waiter_count(1)
        .await;
    state
        .federation
        .suppress_next_terminal_response_publication_for_test();

    let first_responses = state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("durably reject finalized operation before publication interruption");
    let rejected_response = first_responses
        .iter()
        .find(|response| response.operation_id == rejected_operation_id)
        .expect("rejected terminal response");
    assert_eq!(
        rejected_response.outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    let rejection = rejected_response
        .outcome
        .entry()
        .rejection()
        .cloned()
        .expect("persisted Cashu error response");
    assert!(!waiter.is_finished());
    assert_eq!(state.federation_pending_response_waiter_count(), 1);
    assert_eq!(state.federation_response_cache_entry_count(), 0);
    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(rejected_operation_id)
        .await
        .expect("rejected journal lookup")
        .expect("persisted rejected journal entry");
    assert_eq!(persisted.rejection(), Some(&rejection));
    state
        .federation
        .consensus
        .as_ref()
        .expect("consensus")
        .lock()
        .await
        .update_application_state(
            rejected_operation_id,
            FederationApplicationState::Accepted {},
        )
        .expect("simulate interruption before rejected in-memory overlay");

    state
        .apply_federation_aleph_bft_finalized_unit(unit.clone())
        .await
        .expect("duplicate finalized unit republishes rejection");
    let notified = time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("rejected waiter notification timeout")
        .expect("rejected waiter task");
    assert_eq!(notified.operation_id, rejected_operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(notified.outcome.entry().rejection(), Some(&rejection));

    for _ in 0..4 {
        state
            .apply_federation_aleph_bft_finalized_unit(unit.clone())
            .await
            .expect("idempotent duplicate rejection publication");
    }
    assert_eq!(state.federation_pending_response_waiter_count(), 0);
    assert_eq!(state.federation_response_cache_entry_count(), 2);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_reports_state_rejected_swap() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let first_input = proof_for_amount(&state, 1, "aleph-conflict-first");
    let first_output = swap_output(&state, "aleph-conflict-first");
    let first_request =
        cdk::nuts::SwapRequest::new(vec![first_input.clone()], vec![first_output.clone()]);
    let first_envelope = config
        .swap_operation(first_request.clone())
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "aleph-conflict-loser");
    let conflicting_request =
        cdk::nuts::SwapRequest::new(vec![first_input], vec![conflicting_output.clone()]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");

    let later_input = proof_for_amount(&state, 1, "aleph-conflict-later");
    let later_output = swap_output(&state, "aleph-conflict-later");
    let later_request = cdk::nuts::SwapRequest::new(vec![later_input], vec![later_output.clone()]);
    let later_envelope = config
        .swap_operation(later_request.clone())
        .expect("later swap envelope");
    let later_operation_id = later_envelope.operation_id().expect("later operation id");

    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_envelope),
            FederationConsensusItem::operation(conflicting_envelope),
            FederationConsensusItem::operation(later_envelope),
        ],
    )
    .expect("AlephBFT finalized unit");
    let wait_state = state.clone();
    let waiter = tokio::spawn(async move {
        wait_state
            .wait_for_federation_operation_acceptance(conflicting_operation_id)
            .await
            .expect("notified conflicting operation")
    });
    tokio::task::yield_now().await;

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");

    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0].operation_id, first_operation_id);
    assert_eq!(
        responses[0].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(responses[1].operation_id, conflicting_operation_id);
    assert_eq!(
        responses[1].outcome.entry().status(),
        FederationOperationStatus::Rejected
    );
    assert_eq!(responses[2].operation_id, later_operation_id);
    assert_eq!(
        responses[2].outcome.entry().status(),
        FederationOperationStatus::Applied
    );
    let notified = waiter.await.expect("waiter task");
    assert_eq!(notified.operation_id, conflicting_operation_id);
    assert_eq!(
        notified.outcome.entry().status(),
        FederationOperationStatus::Rejected
    );

    let first_response = config
        .swap_share_response(responses[0].outcome.entry(), &first_request)
        .expect("first expected share response");
    let later_response = config
        .swap_share_response(responses[2].outcome.entry(), &later_request)
        .expect("later expected share response");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_output.blinded_secret,
                conflicting_output.blinded_secret,
                later_output.blinded_secret,
            ])
            .await
            .expect("signature lookup"),
        vec![
            Some(first_response.signatures[0].clone()),
            None,
            Some(later_response.signatures[0].clone()),
        ]
    );

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 3);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
    assert_eq!(persisted[2].status(), FederationOperationStatus::Applied);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_applies_rejected_trailing_entry() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let input = proof_for_amount(&state, 1, "aleph-trailing-rejected");
    let first_output = swap_output(&state, "aleph-trailing-rejected-winner");
    let first_request = cdk::nuts::SwapRequest::new(vec![input.clone()], vec![first_output]);
    let first_envelope = config
        .swap_operation(first_request)
        .expect("first swap envelope");
    let first_operation_id = first_envelope.operation_id().expect("first operation id");

    let conflicting_output = swap_output(&state, "aleph-trailing-rejected-loser");
    let conflicting_request = cdk::nuts::SwapRequest::new(vec![input], vec![conflicting_output]);
    let conflicting_envelope = config
        .swap_operation(conflicting_request)
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(first_envelope),
            FederationConsensusItem::operation(conflicting_envelope),
        ],
    )
    .expect("AlephBFT finalized unit");

    let responses = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect("apply finalized unit");
    assert_eq!(
        responses
            .iter()
            .map(|response| response.outcome.entry().status())
            .collect::<Vec<_>>(),
        vec![
            FederationOperationStatus::Applied,
            FederationOperationStatus::Rejected
        ]
    );
    assert_eq!(responses[0].operation_id, first_operation_id);
    assert_eq!(responses[1].operation_id, conflicting_operation_id);

    let persisted = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].status(), FederationOperationStatus::Applied);
    assert_eq!(persisted[1].status(), FederationOperationStatus::Rejected);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_units_serialize_cross_member_value_conflicts() {
    let (public_config, first_private, second_private) = transport_config();
    let first_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("first db"));
    let second_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("second db"));
    let first_mnemonic = Mnemonic::generate(12).expect("first mnemonic");
    let second_mnemonic = Mnemonic::generate(12).expect("second mnemonic");
    let first_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        first_private.clone(),
        first_db,
        &first_mnemonic.to_seed_normalized(""),
    )
    .await;
    let second_state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        second_private.clone(),
        second_db,
        &second_mnemonic.to_seed_normalized(""),
    )
    .await;
    let config = first_state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();

    let mint_quote_id = QuoteId::new();
    let first_mint_output = mint_output(&first_state, &mint_quote_id);
    let conflicting_mint_output = swap_output(&first_state, "aleph-cross-member-mint-loser");
    let first_mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![first_mint_output.clone()],
        signature: None,
    });
    let conflicting_mint_input = MintInput::Single(MintRequest {
        quote: mint_quote_id.clone(),
        outputs: vec![conflicting_mint_output.clone()],
        signature: None,
    });
    let first_mint_envelope = config
        .mint_operation(first_mint_input)
        .expect("first mint envelope");
    let first_mint_operation_id = first_mint_envelope
        .operation_id()
        .expect("first mint operation id");
    let conflicting_mint_envelope = config
        .mint_operation(conflicting_mint_input)
        .expect("conflicting mint envelope");
    let conflicting_mint_operation_id = conflicting_mint_envelope
        .operation_id()
        .expect("conflicting mint operation id");

    let swap_input = proof_for_amount(&first_state, 1, "aleph-cross-member-swap-input");
    let swap_input_y = swap_input.y().expect("swap input y");
    let first_swap_output = swap_output(&first_state, "aleph-cross-member-swap-winner");
    let conflicting_swap_output = swap_output(&first_state, "aleph-cross-member-swap-loser");
    let first_swap_request =
        cdk::nuts::SwapRequest::new(vec![swap_input.clone()], vec![first_swap_output.clone()]);
    let conflicting_swap_request =
        cdk::nuts::SwapRequest::new(vec![swap_input], vec![conflicting_swap_output.clone()]);
    let first_swap_envelope = config
        .swap_operation(first_swap_request)
        .expect("first swap envelope");
    let first_swap_operation_id = first_swap_envelope
        .operation_id()
        .expect("first swap operation id");
    let conflicting_swap_envelope = config
        .swap_operation(conflicting_swap_request)
        .expect("conflicting swap envelope");
    let conflicting_swap_operation_id = conflicting_swap_envelope
        .operation_id()
        .expect("conflicting swap operation id");

    let melt_quote_id = QuoteId::new();
    let melt_quote_envelope = test_melt_quote_operation(
        &config,
        melt_quote_id.clone(),
        cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "aleph cross-member melt".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        }),
    );
    let melt_inputs = vec![
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-a"),
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-b"),
        proof_for_amount(&first_state, 1, "aleph-cross-member-melt-c"),
    ];
    let melt_input_ys = melt_inputs
        .iter()
        .map(|proof| proof.y().expect("melt input y"))
        .collect::<Vec<_>>();
    let mut first_melt_change_output = swap_output(&first_state, "aleph-cross-member-melt-winner");
    first_melt_change_output.amount = Amount::ZERO;
    let mut conflicting_melt_change_output =
        swap_output(&first_state, "aleph-cross-member-melt-loser");
    conflicting_melt_change_output.amount = Amount::ZERO;
    let first_melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs.clone(),
        Some(vec![first_melt_change_output.clone()]),
    );
    let conflicting_melt_request = cdk::nuts::MeltRequest::new(
        melt_quote_id.clone(),
        melt_inputs,
        Some(vec![conflicting_melt_change_output.clone()]),
    );
    let first_melt_envelope = config
        .melt_operation(first_melt_request)
        .expect("first melt envelope");
    let first_melt_operation_id = first_melt_envelope
        .operation_id()
        .expect("first melt operation id");
    let conflicting_melt_envelope = config
        .melt_operation(conflicting_melt_request)
        .expect("conflicting melt envelope");
    let conflicting_melt_operation_id = conflicting_melt_envelope
        .operation_id()
        .expect("conflicting melt operation id");

    let first_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        first_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(mint_quote_envelope(&first_state, &mint_quote_id)),
            FederationConsensusItem::operation(paid_observation_envelope(
                &first_state,
                &mint_quote_id,
            )),
            FederationConsensusItem::operation(first_mint_envelope),
            FederationConsensusItem::operation(first_swap_envelope),
            FederationConsensusItem::operation(melt_quote_envelope),
            FederationConsensusItem::operation(first_melt_envelope),
            FederationConsensusItem::operation(melt_payment_observation_envelope(
                &first_state,
                &melt_quote_id,
                MeltQuoteState::Paid,
                Some(String::new()),
            )),
        ],
    )
    .expect("first AlephBFT finalized unit");
    let second_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        second_private.member_id,
        0,
        0,
        vec![
            FederationConsensusItem::operation(conflicting_mint_envelope),
            FederationConsensusItem::operation(conflicting_swap_envelope),
            FederationConsensusItem::operation(conflicting_melt_envelope),
        ],
    )
    .expect("second AlephBFT finalized unit");

    for state in [&first_state, &second_state] {
        let first_responses = state
            .apply_federation_aleph_bft_finalized_unit(first_unit.clone())
            .await
            .expect("first finalized unit");
        assert_eq!(
            first_responses
                .iter()
                .map(|response| response.outcome.entry().status())
                .collect::<Vec<_>>(),
            vec![FederationOperationStatus::Applied; 7]
        );
        assert_eq!(first_responses[2].operation_id, first_mint_operation_id);
        assert_eq!(first_responses[3].operation_id, first_swap_operation_id);
        assert_eq!(first_responses[5].operation_id, first_melt_operation_id);

        let second_responses = state
            .apply_federation_aleph_bft_finalized_unit(second_unit.clone())
            .await
            .expect("second finalized unit");
        assert_eq!(
            second_responses
                .iter()
                .map(|response| response.outcome.entry().status())
                .collect::<Vec<_>>(),
            vec![FederationOperationStatus::Rejected; 3]
        );
        assert_eq!(
            second_responses[0].operation_id,
            conflicting_mint_operation_id
        );
        assert_eq!(
            second_responses[1].operation_id,
            conflicting_swap_operation_id
        );
        assert_eq!(
            second_responses[2].operation_id,
            conflicting_melt_operation_id
        );
        assert!(matches!(
            second_responses[0]
                .outcome
                .entry()
                .rejection()
                .cloned()
                .map(cdk::Error::from),
            Some(cdk::Error::IssuedQuote | cdk::Error::UnpaidQuote)
        ));
        assert!(matches!(
            second_responses[1]
                .outcome
                .entry()
                .rejection()
                .cloned()
                .map(cdk::Error::from),
            Some(cdk::Error::TokenAlreadySpent)
        ));
        assert!(second_responses[2].outcome.entry().rejection().is_none());

        let replay_responses = state
            .apply_federation_aleph_bft_finalized_unit(second_unit.clone())
            .await
            .expect("replay finalized unit");
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[0],
                conflicting_mint_operation_id,
            ),
            Err(cdk::Error::IssuedQuote | cdk::Error::UnpaidQuote)
        ));
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[1],
                conflicting_swap_operation_id,
            ),
            Err(cdk::Error::TokenAlreadySpent)
        ));
        assert!(matches!(
            crate::validate_federation_submit_response(
                &replay_responses[2],
                conflicting_melt_operation_id,
            ),
            Err(cdk::Error::Federation(
                FederationError::RejectedOperation { .. }
            ))
        ));

        for operation_id in [
            conflicting_mint_operation_id,
            conflicting_swap_operation_id,
            conflicting_melt_operation_id,
        ] {
            assert!(
                state
                    .mint
                    .localstore()
                    .get_completed_operation(&federation_completed_operation_uuid(operation_id))
                    .await
                    .expect("rejected completed-operation lookup")
                    .is_none(),
                "rejected federation operation must not be completed"
            );
        }

        let quote = state
            .mint
            .localstore()
            .get_mint_quote(&mint_quote_id)
            .await
            .expect("mint quote lookup")
            .expect("mint quote");
        assert_eq!(quote.state(), MintQuoteState::Issued);
        assert_eq!(quote.amount_issued(), Amount::new(1, CurrencyUnit::Sat));

        let melt_quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(melt_quote.state, MeltQuoteState::Paid);

        let swap_states = state
            .mint
            .localstore()
            .get_proofs_states(&[swap_input_y])
            .await
            .expect("swap proof state lookup");
        assert_eq!(swap_states, vec![Some(cdk::nuts::State::Spent)]);
        let melt_states = state
            .mint
            .localstore()
            .get_proofs_states(&melt_input_ys)
            .await
            .expect("melt proof state lookup");
        assert_eq!(
            melt_states,
            vec![
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
                Some(cdk::nuts::State::Spent),
            ]
        );

        let signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[
                first_mint_output.blinded_secret,
                conflicting_mint_output.blinded_secret,
                first_swap_output.blinded_secret,
                conflicting_swap_output.blinded_secret,
                first_melt_change_output.blinded_secret,
                conflicting_melt_change_output.blinded_secret,
            ])
            .await
            .expect("signature lookup");
        assert!(signatures[0].is_some());
        assert!(signatures[1].is_none());
        assert!(signatures[2].is_some());
        assert!(signatures[3].is_none());
        assert!(signatures[4].is_some());
        assert!(signatures[5].is_none());

        let persisted = state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("journal lookup");
        assert_eq!(persisted.len(), 10);
        assert_eq!(
            persisted
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            vec![
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Applied,
                FederationOperationStatus::Rejected,
                FederationOperationStatus::Rejected,
                FederationOperationStatus::Rejected,
            ]
        );
        assert!(matches!(
            persisted[7].rejection().cloned().map(cdk::Error::from),
            Some(cdk::Error::IssuedQuote | cdk::Error::UnpaidQuote)
        ));
        assert!(matches!(
            persisted[8].rejection().cloned().map(cdk::Error::from),
            Some(cdk::Error::TokenAlreadySpent)
        ));
        assert!(persisted[9].rejection().is_none());
    }

    let first_entries = first_state
        .federation
        .consensus
        .as_ref()
        .expect("first consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let second_entries = second_state
        .federation
        .consensus
        .as_ref()
        .expect("second consensus")
        .lock()
        .await
        .journal()
        .entries()
        .to_vec();
    let first_digest = materialized_state_digest(&first_state, &first_entries)
        .await
        .expect("first materialized digest");
    let second_digest = materialized_state_digest(&second_state, &second_entries)
        .await
        .expect("second materialized digest");
    assert_eq!(first_digest, second_digest);
}

#[tokio::test]
async fn federation_aleph_bft_finalized_unit_conflict_does_not_mutate_runtime() {
    let (public_config, local_private, peer_private) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db_and_aleph_bft(
        public_config.clone(),
        local_private.clone(),
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let quote_id = QuoteId::new();
    let unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state, &quote_id,
        ))],
    )
    .expect("AlephBFT finalized unit");

    let mut conflicting_adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("conflicting AlephBFT adapter");
    let conflicting_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        local_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state,
            &QuoteId::new(),
        ))],
    )
    .expect("conflicting finalized unit");
    conflicting_adapter
        .finalize_unit(conflicting_unit)
        .expect("conflicting finalized item");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflicting finalized item transaction");
    tx.add_federation_aleph_bft_finalized_items(conflicting_adapter.finalized_items())
        .await
        .expect("persist conflicting finalized item");
    tx.commit()
        .await
        .expect("commit conflicting finalized item");

    let err = state
        .apply_federation_aleph_bft_finalized_unit(unit)
        .await
        .expect_err("conflicting finalized item should reject");
    assert!(
        err.to_string()
            .contains("Conflicting federation AlephBFT finalized item"),
        "unexpected error: {err}"
    );
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after rejected finalization")
        .is_none());
    assert!(state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries after rejected finalization")
        .is_empty());

    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 0);
}
