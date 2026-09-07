use super::*;

#[tokio::test]
async fn federation_runtime_startup_at_trusted_checkpoint_loads_no_prefix_rows() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    state
        .submit_federation_envelope(envelope.clone())
        .await
        .expect("persist checkpointed operation");

    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify checkpoint quorum"),
    )
    .await
    .expect("promote checkpoint");
    tx.commit().await.expect("commit checkpoint promotion");
    let frontier = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("backup frontier");
    let verified = quorum
        .verify(&public_config)
        .expect("verified backup checkpoint");
    let manifest = FederationDatabaseBackupManifest::new(
        &public_config,
        &local_private,
        &verified,
        frontier,
        checkpoint.aleph_bft_session,
        0,
        0,
    )
    .expect("checkpoint-bound backup manifest");
    manifest
        .ensure_matches_current(&manifest)
        .expect("matching backup generation");

    let (checkpoint_consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("checkpoint startup");
    let checkpoint_consensus = checkpoint_consensus.expect("consensus");
    {
        let consensus = checkpoint_consensus.lock().await;
        let status = consensus.status();
        assert_eq!(status.accepted_operation_count, 1);
        assert_eq!(status.finalized_item_count, 1);
        assert_eq!(status.retained_operation_from_index, 1);
        assert_eq!(status.retained_operation_count, 0);
        assert_eq!(status.retained_finalized_item_from_index, 1);
        assert_eq!(status.retained_finalized_item_count, 0);
        assert_eq!(status.retained_finalized_canonical_bytes, 0);
        assert!(consensus.journal().entries().is_empty());
        assert!(consensus.finalized_items().is_empty());
        assert_eq!(
            consensus.journal().latest_order_digest(),
            checkpoint.journal_order_digest
        );
    }

    let mut restarted_state = state.clone();
    restarted_state.federation.consensus = Some(checkpoint_consensus);
    let mut conflicting = mint_quote_envelope(&restarted_state, &quote_id);
    let FederationOperation::MintQuote(operation) = &mut conflicting.operation else {
        panic!("mint quote operation");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("bolt11 mint quote response");
    };
    response.request.push_str("-conflict");
    conflicting = conflicting
        .authorize(&local_private)
        .expect("authorize conflicting operation");
    let conflict_payload = {
        let consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("checkpoint consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::operation(conflicting)],
        )
        .expect("conflicting finalized payload")
    };
    let conflict = restarted_state
        .federation
        .apply_finalized_unit(conflict_payload)
        .await
        .expect("finalize pre-checkpoint conflict")
        .pop()
        .expect("conflict response");
    assert!(matches!(
        conflict.outcome,
        FederationJournalAppendOutcome::Appended(ref entry)
            if matches!(
                entry.application,
                FederationApplicationState::ConsensusRejected {}
            )
    ));

    let (suffix_consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("checkpoint suffix startup before duplicate");
    restarted_state.federation.consensus = suffix_consensus;
    let duplicate_payload = {
        let consensus = restarted_state
            .federation
            .consensus
            .as_ref()
            .expect("suffix consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("duplicate finalized payload")
    };
    let duplicate = restarted_state
        .federation
        .apply_finalized_unit(duplicate_payload.clone())
        .await
        .expect("finalize pre-checkpoint duplicate")
        .pop()
        .expect("duplicate response");
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(ref entry) if entry.index == 0
    ));

    restarted_state
        .submit_federation_envelope(mint_quote_envelope(&restarted_state, &QuoteId::new()))
        .await
        .expect("persist post-checkpoint suffix");
    let (consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("checkpoint suffix startup");
    restarted_state.federation.consensus = consensus;
    let replayed_duplicate = restarted_state
        .federation
        .apply_finalized_unit(duplicate_payload)
        .await
        .expect("replay checkpoint suffix duplicate")
        .pop()
        .expect("replayed duplicate response");
    assert!(matches!(
        replayed_duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(ref entry) if entry.index == 0
    ));
    let consensus = restarted_state
        .federation
        .consensus
        .as_ref()
        .expect("suffix consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.status().accepted_operation_count, 3);
    assert_eq!(consensus.status().finalized_item_count, 4);
    assert_eq!(consensus.journal().entries().len(), 2);
    assert_eq!(consensus.journal().entries()[0].index, 1);
    assert_eq!(consensus.journal().entries()[1].index, 2);
    assert_eq!(consensus.finalized_items().len(), 3);
    assert_eq!(consensus.finalized_items()[0].accepted_item_index, 1);
    assert_eq!(
        consensus.finalized_items()[1].status,
        FederationAlephBftFinalizedItemStatus::DuplicateOperation
    );
    assert_eq!(consensus.finalized_items()[2].accepted_item_index, 3);
}

async fn assert_trusted_checkpoint_partial_tail_restart_recovers(
    terminal_suffix_prefix: usize,
    mark_first_entry_failed: bool,
) {
    let (state, public_config, peer_private) = federation_route_state().await;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("persist trusted checkpoint base operation");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin trusted checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify trusted checkpoint quorum"),
    )
    .await
    .expect("promote trusted checkpoint");
    tx.commit().await.expect("commit trusted checkpoint");

    let suffix_quote_ids = [QuoteId::new(), QuoteId::new()];
    let payload = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("checkpoint crash consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            suffix_quote_ids
                .iter()
                .map(|quote_id| {
                    FederationConsensusItem::operation(mint_quote_envelope(&state, quote_id))
                })
                .collect(),
        )
        .expect("checkpoint crash suffix payload")
    };
    let (next_consensus, finalized_items, entries) = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("checkpoint crash consensus")
            .lock()
            .await;
        let mut next_consensus = consensus.clone();
        let finalized_start = next_consensus
            .aleph_bft_finalized_items()
            .expect("checkpoint crash finalized items")
            .len();
        let responses = next_consensus
            .finalize_aleph_bft_unit(payload)
            .expect("finalize checkpoint crash suffix");
        let finalized_items = next_consensus
            .aleph_bft_finalized_items()
            .expect("checkpoint crash finalized suffix")[finalized_start..]
            .to_vec();
        let entries = responses
            .into_iter()
            .filter_map(|response| match response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        (next_consensus, finalized_items, entries)
    };
    assert_eq!(entries.len(), 2);
    state
        .federation
        .persist_finalization(&finalized_items, &entries)
        .await
        .expect("persist checkpoint crash suffix before application");
    *state
        .federation
        .consensus
        .as_ref()
        .expect("checkpoint crash consensus")
        .lock()
        .await = next_consensus;
    state
        .federation
        .apply_journal_entries(&entries[..terminal_suffix_prefix])
        .await
        .expect("apply checkpoint crash terminal prefix");
    if mark_first_entry_failed {
        assert_eq!(terminal_suffix_prefix, 0);
        let mut tx = db
            .begin_transaction()
            .await
            .expect("begin failed checkpoint suffix transaction");
        tx.set_federation_operation_application_state(
            entries[0].operation_id,
            &cdk::federation::FederationApplicationState::Failed {},
        )
        .await
        .expect("mark checkpoint suffix operation failed");
        tx.commit().await.expect("commit failed checkpoint suffix");
    }

    let frontier = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("checkpoint crash durable frontier");
    assert_eq!(
        frontier.journal_next_index,
        checkpoint.journal_next_index + 2
    );
    assert_eq!(
        frontier.finalized_next_item_index,
        checkpoint.finalized_next_item_index + 2
    );
    assert_eq!(
        frontier.application_next_index,
        checkpoint.journal_next_index + terminal_suffix_prefix as u64
    );

    let (restarted, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("trusted-checkpoint startup repairs terminal application");
    let repaired_frontier = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("repaired checkpoint frontier");
    assert_eq!(
        repaired_frontier.application_next_index,
        repaired_frontier.journal_next_index
    );
    let repaired_entries = db
        .get_federation_journal_entries_from(checkpoint.journal_next_index)
        .await
        .expect("repaired checkpoint journal suffix");
    assert_eq!(repaired_entries.len(), suffix_quote_ids.len());
    assert!(repaired_entries.iter().all(|entry| {
        entry.application == cdk::federation::FederationApplicationState::Applied {}
    }));
    let expected_application_digest = repaired_entries
        .iter()
        .try_fold(checkpoint.application_digest, |digest, entry| {
            digest.append_terminal(entry.index, entry.operation_id, &entry.application)
        })
        .expect("repaired application digest");
    assert_eq!(
        repaired_frontier.application_digest,
        expected_application_digest
    );
    for quote_id in &suffix_quote_ids {
        assert!(db
            .get_mint_quote(quote_id)
            .await
            .expect("repaired suffix quote lookup")
            .is_some());
    }
    let restarted = restarted.expect("repaired checkpoint consensus");
    let restarted = restarted.lock().await;
    assert_eq!(restarted.journal().entries().len(), suffix_quote_ids.len());
    assert!(restarted.journal().entries().iter().all(|entry| {
        entry.application == cdk::federation::FederationApplicationState::Applied {}
    }));
    drop(restarted);

    let snapshot_after_repair = materialized_state_snapshot(&state)
        .await
        .expect("materialized state after checkpoint repair");
    crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("second startup uses terminal checkpoint tail");
    assert_eq!(
        materialized_state_snapshot(&state)
            .await
            .expect("materialized state after second startup"),
        snapshot_after_repair
    );
}

#[tokio::test]
async fn trusted_checkpoint_restart_recovers_suffix_before_first_application() {
    assert_trusted_checkpoint_partial_tail_restart_recovers(0, false).await;
}

#[tokio::test]
async fn trusted_checkpoint_restart_recovers_partially_applied_suffix() {
    assert_trusted_checkpoint_partial_tail_restart_recovers(1, false).await;
}

#[tokio::test]
async fn trusted_checkpoint_restart_retries_failed_suffix_application() {
    assert_trusted_checkpoint_partial_tail_restart_recovers(0, true).await;
}

async fn assert_trusted_checkpoint_suffix_rejects_corruption(alter_consensus_evidence: bool) {
    let (state, public_config, peer_private) = federation_route_state().await;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("apply corruption checkpoint base");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin corruption checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify corruption checkpoint quorum"),
    )
    .await
    .expect("promote corruption checkpoint");
    tx.commit().await.expect("commit corruption checkpoint");

    let (journal_entry, finalized_items) = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("corruption checkpoint consensus")
            .lock()
            .await;
        let status = consensus.status();
        let accepted_envelope = mint_quote_envelope(&state, &QuoteId::new());
        let accepted_payload = FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::operation(accepted_envelope)],
        )
        .expect("accepted corruption payload");
        let mut accepted_runtime = consensus.clone();
        let finalized_start = accepted_runtime.finalized_items().len();
        let accepted_response = accepted_runtime
            .finalize_unit(accepted_payload)
            .expect("finalize accepted corruption payload")
            .into_iter()
            .next()
            .expect("accepted corruption response");
        let finalized_items = accepted_runtime.finalized_items()[finalized_start..].to_vec();
        let mut journal_entry = accepted_response.outcome.entry().clone();
        if alter_consensus_evidence {
            let evidence = journal_entry
                .consensus_evidence
                .as_mut()
                .expect("accepted consensus evidence");
            evidence.unit_index = evidence.unit_index.saturating_add(1);
        } else {
            let changed_payload = FederationAlephBftUnitPayload::new(
                status.federation_id,
                status.local_member,
                status.session,
                consensus.next_unit_index(),
                vec![FederationConsensusItem::operation(mint_quote_envelope(
                    &state,
                    &QuoteId::new(),
                ))],
            )
            .expect("changed corruption payload");
            let mut changed_runtime = consensus.clone();
            journal_entry = changed_runtime
                .finalize_unit(changed_payload)
                .expect("finalize changed corruption payload")
                .into_iter()
                .next()
                .expect("changed corruption response")
                .outcome
                .entry()
                .clone();
        }
        (journal_entry, finalized_items)
    };
    let corrupted_operation_id = journal_entry.operation_id;
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin corrupted checkpoint suffix transaction");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist corrupted checkpoint finalized suffix");
    tx.add_federation_journal_entry(&journal_entry)
        .await
        .expect("persist corrupted checkpoint journal suffix");
    tx.commit()
        .await
        .expect("commit corrupted checkpoint suffix");

    let error = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect_err("corrupted trusted-checkpoint suffix must fail closed");
    assert!(matches!(
        error.downcast_ref::<cdk::Error>(),
        Some(cdk::Error::Federation(
            FederationError::AlephBftFinalizedItemRecoveryMismatch { .. }
                | FederationError::CatchUpOperationIdMismatch { .. }
        ))
    ));
    assert!(db
        .get_federation_journal_entry_by_operation_id(corrupted_operation_id)
        .await
        .expect("corrupted operation lookup")
        .is_some());
    assert_eq!(
        db.get_federation_checkpoint_frontier()
            .await
            .expect("corrupted checkpoint frontier")
            .application_next_index,
        checkpoint.journal_next_index
    );
}

#[tokio::test]
async fn trusted_checkpoint_restart_rejects_changed_suffix_envelope() {
    assert_trusted_checkpoint_suffix_rejects_corruption(false).await;
}

#[tokio::test]
async fn trusted_checkpoint_restart_rejects_changed_suffix_consensus_evidence() {
    assert_trusted_checkpoint_suffix_rejects_corruption(true).await;
}

#[tokio::test]
async fn trusted_checkpoint_startup_rejects_changed_terminal_result() {
    let (state, public_config, peer_private) = federation_route_state().await;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("apply terminal-result checkpoint base");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin terminal-result checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify terminal-result checkpoint quorum"),
    )
    .await
    .expect("promote terminal-result checkpoint");
    tx.commit()
        .await
        .expect("commit terminal-result checkpoint");
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("apply terminal-result checkpoint suffix");

    let frontier = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("terminal-result checkpoint frontier");
    let mut journal_entries = db
        .get_federation_journal_entries_from(checkpoint.journal_next_index)
        .await
        .expect("terminal-result checkpoint journal suffix");
    let finalized_items = db
        .get_federation_aleph_bft_finalized_items_from(checkpoint.finalized_next_item_index)
        .await
        .expect("terminal-result checkpoint finalized suffix");
    assert_eq!(journal_entries.len(), 1);
    journal_entries[0].application = cdk::federation::FederationApplicationState::Rejected {
        response: cdk::error::ErrorResponse::from(cdk::Error::TokenAlreadySpent),
    };

    let active_session = db
        .get_federation_aleph_bft_active_session()
        .await
        .expect("active session")
        .unwrap_or(0);
    let verified_quorum = quorum
        .verify(&public_config)
        .expect("verify checkpoint quorum");
    let verifier = crate::FederationCheckpointStartupVerifier {
        mint: &state.mint,
        config: &public_config,
        member_id: local_private.member_id,
        active_session,
        checkpoint: &verified_quorum,
    };
    let error = verifier
        .verify(&frontier, &journal_entries, &finalized_items)
        .await
        .expect_err("changed terminal result must fail suffix verification");
    assert!(matches!(
        error,
        cdk::Error::Federation(FederationError::AlephBftFinalizedItemRecoveryMismatch { .. })
    ));
}

#[tokio::test]
async fn trusted_checkpoint_restart_replays_rejection_and_later_operation() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let config = state
        .mint
        .federation_config()
        .expect("federation config")
        .clone();
    let spent_input = proof_for_amount(&state, 1, "checkpoint-replay-conflict");
    let first_request = cdk::nuts::SwapRequest::new(
        vec![spent_input.clone()],
        vec![swap_output(&state, "checkpoint-replay-first")],
    );
    state
        .submit_federation_envelope(
            config
                .swap_operation(first_request)
                .expect("first swap envelope"),
        )
        .await
        .expect("apply checkpoint base swap");

    let local_private = config.private_config().clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin rejection checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify rejection checkpoint quorum"),
    )
    .await
    .expect("promote rejection checkpoint");
    tx.commit().await.expect("commit rejection checkpoint");

    let conflicting_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![spent_input],
            vec![swap_output(&state, "checkpoint-replay-conflicting")],
        ))
        .expect("conflicting swap envelope");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("conflicting operation id");
    let later_input = proof_for_amount(&state, 1, "checkpoint-replay-later");
    let later_input_y = later_input.y().expect("later input y");
    let later_envelope = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![later_input],
            vec![swap_output(&state, "checkpoint-replay-later")],
        ))
        .expect("later swap envelope");
    let later_operation_id = later_envelope.operation_id().expect("later operation id");
    let payload = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("rejection checkpoint consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![
                FederationConsensusItem::operation(conflicting_envelope),
                FederationConsensusItem::operation(later_envelope),
            ],
        )
        .expect("rejection checkpoint suffix payload")
    };
    let (finalized_items, entries) = {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("rejection checkpoint consensus")
            .lock()
            .await;
        let mut next_consensus = consensus.clone();
        let finalized_start = next_consensus.finalized_items().len();
        let responses = next_consensus
            .finalize_unit(payload)
            .expect("finalize rejection checkpoint suffix");
        let finalized_items = next_consensus.finalized_items()[finalized_start..].to_vec();
        let entries = responses
            .into_iter()
            .filter_map(|response| match response.outcome {
                FederationJournalAppendOutcome::Appended(entry) => Some(entry),
                FederationJournalAppendOutcome::Duplicate(_) => None,
            })
            .collect::<Vec<_>>();
        (finalized_items, entries)
    };
    state
        .federation
        .persist_finalization(&finalized_items, &entries)
        .await
        .expect("persist rejection checkpoint suffix before application");

    crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("startup replays rejection checkpoint suffix");
    let repaired = db
        .get_federation_journal_entries_from(checkpoint.journal_next_index)
        .await
        .expect("repaired rejection checkpoint suffix");
    assert_eq!(repaired.len(), 2);
    assert_eq!(repaired[0].operation_id, conflicting_operation_id);
    assert_eq!(repaired[0].status(), FederationOperationStatus::Rejected);
    let rejection = repaired[0]
        .rejection()
        .expect("standard deterministic rejection response")
        .clone();
    assert_eq!(repaired[1].operation_id, later_operation_id);
    assert_eq!(repaired[1].status(), FederationOperationStatus::Applied);
    assert_eq!(
        db.get_proofs_states(&[later_input_y])
            .await
            .expect("later proof state"),
        vec![Some(cdk::nuts::State::Spent)]
    );
    let frontier = db
        .get_federation_checkpoint_frontier()
        .await
        .expect("rejection checkpoint frontier");
    assert_eq!(frontier.application_next_index, frontier.journal_next_index);
    let application_digest = frontier.application_digest;

    crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("second rejection checkpoint startup");
    let stable_rejection = db
        .get_federation_journal_entry_by_operation_id(conflicting_operation_id)
        .await
        .expect("stable rejection lookup")
        .expect("stable rejected operation");
    assert_eq!(stable_rejection.rejection(), Some(&rejection));
    assert_eq!(
        db.get_federation_checkpoint_frontier()
            .await
            .expect("stable rejection checkpoint frontier")
            .application_digest,
        application_digest
    );
}

#[tokio::test]
async fn scheduled_checkpoint_captures_first_complete_eligible_boundary() {
    #[cfg(feature = "prometheus")]
    let capture_attempts_before = prometheus_metric_value(
        "cdk_federation_performance_work_total",
        &[("work", "scheduled_capture_attempts")],
    );
    #[cfg(feature = "prometheus")]
    let ineligible_before = prometheus_metric_value(
        "cdk_federation_performance_work_total",
        &[("work", "scheduled_capture_skipped_ineligible")],
    );
    #[cfg(feature = "prometheus")]
    let created_before = prometheus_metric_value(
        "cdk_federation_performance_work_total",
        &[("work", "scheduled_capture_created")],
    );
    #[cfg(feature = "prometheus")]
    let persistence_before = prometheus_metric_value(
        "cdk_federation_performance_work_total",
        &[("work", "checkpoint_persistence_successes")],
    );
    let (mut public_config, local_private, _) = transport_config();
    public_config.checkpoint_policy =
        FederationCheckpointPolicy::new(2, 2).expect("checkpoint policy");
    let state = federation_route_state_for_private(public_config, local_private).await;
    let db = state.mint.localstore();

    time::timeout(
        Duration::from_secs(15),
        state.submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new())),
    )
    .await
    .expect("first finalized operation timeout")
    .expect("first finalized operation");
    assert!(db
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("first scheduled checkpoint lookup")
        .is_none());

    time::timeout(
        Duration::from_secs(15),
        state.submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new())),
    )
    .await
    .expect("eligible finalized operation timeout")
    .expect("eligible finalized operation");
    let candidate = db
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("eligible scheduled checkpoint lookup")
        .expect("scheduled checkpoint candidate");
    let candidate_id = candidate.id().expect("scheduled checkpoint id");
    assert_eq!(candidate.journal_next_index, 2);
    assert_eq!(candidate.finalized_next_item_index, 2);
    assert_eq!(candidate.next_unit_finalization_index, 2);
    assert_eq!(
        candidate.materialized_state_digest,
        state
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("eligible materialized state")
            .digest()
            .expect("eligible materialized state digest")
    );

    time::timeout(
        Duration::from_secs(15),
        state.submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new())),
    )
    .await
    .expect("later finalized operation timeout")
    .expect("later finalized operation");
    let retained = db
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("retained scheduled checkpoint lookup")
        .expect("retained scheduled checkpoint candidate");
    assert_eq!(retained.id().expect("retained checkpoint id"), candidate_id);
    assert_eq!(retained.journal_next_index, 2);
    assert_eq!(
        db.get_federation_checkpoint_signatures(candidate_id)
            .await
            .expect("scheduled checkpoint signatures")
            .len(),
        1
    );
    #[cfg(feature = "prometheus")]
    {
        assert!(
            prometheus_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "scheduled_capture_attempts")],
            ) >= capture_attempts_before + 3.0
        );
        assert!(
            prometheus_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "scheduled_capture_skipped_ineligible")],
            ) >= ineligible_before + 1.0
        );
        assert!(
            prometheus_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "scheduled_capture_created")],
            ) >= created_before + 1.0
        );
        assert!(
            prometheus_metric_value(
                "cdk_federation_performance_work_total",
                &[("work", "checkpoint_persistence_successes")],
            ) >= persistence_before + 1.0
        );
    }
}

#[tokio::test]
async fn scheduled_checkpoint_preserves_the_canonical_byte_trigger() {
    let (mut public_config, local_private, _) = transport_config();
    public_config.checkpoint_policy = FederationCheckpointPolicy {
        checkpoint_interval_journal_operations:
            cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_JOURNAL_OPERATIONS,
        checkpoint_interval_finalized_items:
            cdk::federation::FEDERATION_MAX_CHECKPOINT_BATCH_FINALIZED_ITEMS,
        checkpoint_interval_canonical_bytes: 1,
    };
    let state = federation_route_state_for_private(public_config, local_private).await;

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("byte-triggered finalized operation");

    let candidate = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
        .await
        .expect("byte-triggered checkpoint lookup")
        .expect("byte-triggered scheduled checkpoint");
    assert_eq!(candidate.journal_next_index, 1);
    assert_eq!(candidate.finalized_next_item_index, 1);
}

async fn scheduled_checkpoint_agreement_for_payload<F>(
    checkpoint_policy: FederationCheckpointPolicy,
    make_items: F,
) -> FederationCheckpoint
where
    F: FnOnce(&MintState) -> Vec<FederationConsensusItem>,
{
    let (mut public_config, first_private, second_private) = transport_config();
    public_config.checkpoint_policy = checkpoint_policy;
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let items = make_items(&first);
    apply_identical_finalized_unit(&first, &second, items).await;

    let first_candidate = first
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("first scheduled-boundary lookup")
        .expect("first scheduled-boundary candidate");
    let second_candidate = second
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("second scheduled-boundary lookup")
        .expect("second scheduled-boundary candidate");
    assert_eq!(first_candidate, second_candidate);
    let checkpoint_id = first_candidate.id().expect("scheduled-boundary id");
    let mut signatures = first
        .mint
        .localstore()
        .get_federation_checkpoint_signatures(checkpoint_id)
        .await
        .expect("first scheduled-boundary signature");
    signatures.extend(
        second
            .mint
            .localstore()
            .get_federation_checkpoint_signatures(checkpoint_id)
            .await
            .expect("second scheduled-boundary signature"),
    );
    assert_eq!(signatures.len(), 2);
    assert_eq!(signatures[0].signer, first_private.member_id);
    assert_eq!(signatures[1].signer, second_private.member_id);
    FederationCheckpointQuorum::new(first_candidate.clone(), signatures)
        .verify(&public_config)
        .expect("scheduled-boundary quorum");
    assert_eq!(
        first_candidate.materialized_state_digest,
        first
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("first scheduled-boundary materialized state")
            .digest()
            .expect("first scheduled-boundary materialized digest")
    );
    assert_eq!(
        first_candidate.materialized_state_digest,
        second
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("second scheduled-boundary materialized state")
            .digest()
            .expect("second scheduled-boundary materialized digest")
    );
    first_candidate
}

#[tokio::test]
async fn scheduled_checkpoint_agrees_after_journal_interval_crossed_mid_unit() {
    let candidate = scheduled_checkpoint_agreement_for_payload(
        FederationCheckpointPolicy::new(2, 100).expect("checkpoint policy"),
        |state| {
            (0..3)
                .map(|_| {
                    FederationConsensusItem::operation(mint_quote_envelope(state, &QuoteId::new()))
                })
                .collect()
        },
    )
    .await;

    assert_eq!(candidate.journal_next_index, 3);
    assert_eq!(candidate.finalized_next_item_index, 3);
    assert_eq!(candidate.next_unit_finalization_index, 1);
}

#[tokio::test]
async fn scheduled_checkpoint_agrees_when_only_finalized_interval_is_crossed() {
    let candidate = scheduled_checkpoint_agreement_for_payload(
        FederationCheckpointPolicy::new(100, 2).expect("checkpoint policy"),
        |state| {
            let envelope = mint_quote_envelope(state, &QuoteId::new());
            vec![
                FederationConsensusItem::operation(envelope.clone()),
                FederationConsensusItem::operation(envelope),
            ]
        },
    )
    .await;

    assert_eq!(candidate.journal_next_index, 1);
    assert_eq!(candidate.finalized_next_item_index, 2);
    assert_eq!(candidate.next_unit_finalization_index, 1);
}

#[tokio::test]
async fn startup_recovers_an_interrupted_eligible_checkpoint_capture_once() {
    let (mut public_config, local_private, _) = transport_config();
    public_config.checkpoint_policy =
        FederationCheckpointPolicy::new(2, 2).expect("checkpoint policy");
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
    let seed = [62_u8; 64];
    let initial = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::clone(&db),
        &seed,
    )
    .await;
    initial
        .submit_federation_envelope(mint_quote_envelope(&initial, &QuoteId::new()))
        .await
        .expect("first pre-restart operation");
    initial
        .federation
        .suppress_next_scheduled_checkpoint_capture();
    initial
        .submit_federation_envelope(mint_quote_envelope(&initial, &QuoteId::new()))
        .await
        .expect("eligible pre-restart operation");
    assert!(db
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("pre-restart scheduled checkpoint lookup")
        .is_none());
    drop(initial);

    let mut restarted = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        Arc::clone(&db),
        &seed,
    )
    .await;
    let (consensus, sequences) = crate::create_federation_runtime_state(&restarted.mint)
        .await
        .expect("restart federation runtime");
    restarted.federation.consensus = consensus;
    restarted.federation.sequences = sequences;

    restarted
        .federation
        .recover_scheduled_checkpoint_candidate_at_startup()
        .await
        .expect("recover interrupted checkpoint capture");
    let candidate = db
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Scheduled)
        .await
        .expect("recovered scheduled checkpoint lookup")
        .expect("recovered scheduled checkpoint");
    assert_eq!(candidate.journal_next_index, 2);
    assert_eq!(candidate.finalized_next_item_index, 2);
    let checkpoint_id = candidate.id().expect("recovered checkpoint id");

    restarted
        .federation
        .recover_scheduled_checkpoint_candidate_at_startup()
        .await
        .expect("repeat startup recovery");
    assert_eq!(
        db.get_federation_checkpoint_signatures(checkpoint_id)
            .await
            .expect("recovered checkpoint signatures")
            .len(),
        1
    );
}

#[tokio::test]
async fn startup_checkpoint_capture_skips_scheduled_quorum_ahead_of_local_frontier() {
    let (mut public_config, first_private, second_private) = transport_config();
    public_config.checkpoint_policy =
        FederationCheckpointPolicy::new(1, 1).expect("checkpoint policy");
    let source =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    source
        .submit_federation_envelope(mint_quote_envelope(&source, &QuoteId::new()))
        .await
        .expect("source checkpointed operation");
    let checkpoint = source
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
        .await
        .expect("source scheduled checkpoint lookup")
        .expect("source scheduled checkpoint");
    let signatures = [&first_private, &second_private]
        .into_iter()
        .map(|signer| {
            FederationCheckpointSignature::new_signed(&checkpoint, signer)
                .expect("scheduled checkpoint signature")
        })
        .collect();
    let quorum = FederationCheckpointQuorum::new(checkpoint, signatures);
    quorum
        .verify(&public_config)
        .expect("scheduled checkpoint quorum");

    let lagging = federation_route_state_for_private(public_config, second_private.clone()).await;
    lagging
        .federation
        .persist_checkpoint_quorum(&quorum)
        .await
        .expect("persist ahead scheduled quorum");
    let frontier = lagging
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("lagging frontier");
    assert_eq!(frontier.journal_next_index, 0);

    lagging
        .federation
        .recover_scheduled_checkpoint_candidate_at_startup()
        .await
        .expect("ahead scheduled quorum is not a startup error");
}

#[cfg(feature = "prometheus")]
async fn assert_unchanged_checkpoint_polls_do_not_repeat_signature_verification(
    rounds: usize,
    round_delay: Duration,
    synthetic_operation_count: Option<u64>,
) {
    let local_member = member_id(61_001);
    let peer_member = member_id(61_002);
    let (public_config, local_private, peer_private) =
        transport_config_with_members_and_federation_api_urls(
            local_member,
            peer_member,
            MintUrl::from_str("http://checkpoint-idle-local.test")
                .expect("local federation api url"),
            MintUrl::from_str("http://checkpoint-idle-peer.test").expect("peer federation api url"),
        );
    let mut state =
        federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
    let checkpoint = match synthetic_operation_count {
        Some(operation_count) => {
            build_synthetic_checkpoint_history(
                &mut state,
                &public_config,
                &local_private,
                &peer_private,
                operation_count,
            )
            .await
        }
        None => {
            state
                .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
                .await
                .expect("persist checkpointed operation");
            let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
                &state,
                &public_config,
                None,
                &[&local_private, &peer_private],
            )
            .await;
            let db = state.mint.localstore();
            let mut tx = db
                .begin_transaction()
                .await
                .expect("begin trusted checkpoint promotion");
            tx.promote_verified_federation_checkpoint(
                &quorum
                    .verify(&public_config)
                    .expect("verify trusted checkpoint quorum"),
            )
            .await
            .expect("promote trusted checkpoint");
            tx.commit().await.expect("commit trusted checkpoint");
            checkpoint
        }
    };
    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("restart idle checkpoint member");
    state.federation.consensus = consensus;
    state.federation.sequences = sequences;
    {
        let consensus = state
            .federation
            .consensus
            .as_ref()
            .expect("restarted idle checkpoint consensus")
            .lock()
            .await;
        assert_eq!(
            consensus.status().accepted_operation_count,
            synthetic_operation_count.unwrap_or(1)
        );
        assert!(consensus.journal().entries().is_empty());
        assert!(consensus.finalized_items().is_empty());
    }

    let mut peer_summary = state
        .federation
        .checkpoint_summary(Some(0))
        .await
        .expect("local checkpoint summary");
    peer_summary.member_id = peer_member;
    let transport = MockFederationTransport::default()
        .with_checkpoint_summary_response(peer_summary.clone(), peer_private);
    let client = FederationRestPeerClient::new(transport.clone(), &public_config, local_private)
        .expect("checkpoint peer client");
    let peer = public_config.member(peer_member).expect("checkpoint peer");
    let member_label = local_member.to_string();
    let labels = [
        ("member", member_label.as_str()),
        ("work", "signatures_verified"),
    ];
    let before = prometheus_metric_value("cdk_federation_checkpoint_work_total", &labels);
    let member_labels = [("member", member_label.as_str())];
    let total_work_before =
        prometheus_metric_sum("cdk_federation_checkpoint_work_total", &member_labels);
    let audit_labels = [
        ("member", member_label.as_str()),
        ("operation", "materialized_state_digest"),
    ];
    let audits_before = prometheus_metric_value(
        "cdk_federation_checkpoint_duration_seconds_count",
        &audit_labels,
    );
    let captured_logs = CapturedLogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .with_target(false)
        .with_ansi(false)
        .with_writer(captured_logs.clone())
        .finish();
    let rounds_started_at = std::time::Instant::now();
    async {
        for _ in 0..rounds {
            assert!(sync_loop::next_local_checkpoint_catch_up_target(&state)
                .await
                .expect("inspect unchanged local checkpoint target")
                .is_none());
            if !round_delay.is_zero() {
                time::sleep(round_delay).await;
            }
            let summary = state
                .federation
                .checkpoint_summary(Some(0))
                .await
                .expect("unchanged local checkpoint summary");
            assert_eq!(
                summary.latest_recovery_quorum,
                peer_summary.latest_recovery_quorum
            );
            let fetched = state
                .federation
                .fetch_checkpoint(checkpoint.id().expect("checkpoint id"))
                .await
                .expect("unchanged local checkpoint fetch");
            assert!(fetched.quorum.is_some());
            assert_eq!(
                exchange_checkpoint_round(&state, &client, std::slice::from_ref(&peer)).await,
                0
            );
        }
        state.mark_federation_healthy().await;
        assert_eq!(
            run_federation_peer_sync_round(&state, &client, &[]).await,
            0
        );
    }
    .with_subscriber(subscriber)
    .await;

    assert_eq!(
        prometheus_metric_value("cdk_federation_checkpoint_work_total", &labels),
        before
    );
    assert_eq!(
        prometheus_metric_sum("cdk_federation_checkpoint_work_total", &member_labels,),
        total_work_before
    );
    assert_eq!(
        prometheus_metric_value(
            "cdk_federation_checkpoint_duration_seconds_count",
            &audit_labels,
        ),
        audits_before
    );
    assert_eq!(transport.checkpoint_summary_requests().len(), rounds);
    assert!(transport.catch_up_requests().is_empty());
    let captured_logs =
        String::from_utf8(captured_logs.0.lock().expect("captured log lock").clone())
            .expect("captured logs are UTF-8");
    assert!(
        captured_logs.is_empty(),
        "unchanged checkpoint polls emitted logs: {captured_logs}"
    );
    if !round_delay.is_zero() {
        eprintln!(
            "checkpoint_idle history_operations={} rounds={} interval_ms={} elapsed_ms={} checkpoint_work_delta=0 signatures_verified_delta=0 materialized_state_audits_delta=0 catch_up_requests=0 emitted_logs=0",
            synthetic_operation_count.unwrap_or(1),
            rounds,
            round_delay.as_millis(),
            rounds_started_at.elapsed().as_millis()
        );
    }
}

#[cfg(feature = "prometheus")]
#[tokio::test]
async fn unchanged_checkpoint_polls_do_not_repeat_signature_verification() {
    assert_unchanged_checkpoint_polls_do_not_repeat_signature_verification(
        12,
        Duration::ZERO,
        None,
    )
    .await;
}

#[cfg(feature = "prometheus")]
#[tokio::test]
#[ignore = "manual release-mode ten-minute long-history idle checkpoint benchmark"]
async fn benchmark_unchanged_checkpoint_idle_rounds() {
    assert_unchanged_checkpoint_polls_do_not_repeat_signature_verification(
        120,
        Duration::from_secs(5),
        Some(100_000),
    )
    .await;
}

#[tokio::test]
async fn checkpoint_audit_route_rejects_a_remote_member_request() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_member = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        local_member,
        1,
        FederationRestMessageKind::CheckpointAudit,
        FederationCheckpointAuditRequest::new(),
        &peer_private,
    )
    .expect("remote member audit request");

    let response = post_checkpoint_audit(State(state), Json(request))
        .await
        .expect_err("remote member must not trigger a local full-state audit");

    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn operator_checkpoint_audit_matches_exact_trusted_frontier() {
    let (state, public_config, peer_private) = federation_route_state().await;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("persist checkpointed operation");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify checkpoint quorum"),
    )
    .await
    .expect("promote checkpoint");
    tx.commit().await.expect("commit checkpoint promotion");
    let checkpoint_id = checkpoint.id().expect("checkpoint id");
    assert_eq!(
        state
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("unaudited promoted frontier")
            .latest_audited_checkpoint_id,
        None
    );

    let audit = state
        .federation
        .audit_trusted_checkpoint_materialized_state()
        .await
        .expect("operator checkpoint audit");

    assert_eq!(audit.checkpoint.checkpoint_id, checkpoint_id);
    assert_eq!(
        audit.materialized_state_digest,
        checkpoint.materialized_state_digest
    );
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("operator-audited frontier");
    assert_eq!(frontier.latest_audited_checkpoint_id, Some(checkpoint_id));
    assert_eq!(
        frontier.latest_materialized_state_digest,
        Some(checkpoint.materialized_state_digest)
    );
}

#[tokio::test]
async fn operator_checkpoint_audit_latches_materialized_state_mismatch() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let quote_id = QuoteId::new();
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &quote_id))
        .await
        .expect("persist checkpointed operation");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (_checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify checkpoint quorum"),
    )
    .await
    .expect("promote checkpoint");
    tx.commit().await.expect("commit checkpoint promotion");

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin out-of-band quote mutation");
    let mut quote = tx
        .get_mint_quote(&quote_id)
        .await
        .expect("load mint quote")
        .expect("mint quote");
    quote
        .add_payment(
            Amount::from(1).with_unit(CurrencyUnit::Sat),
            "out-of-band-audit-payment".to_string(),
            Some(1),
        )
        .expect("alter mint quote");
    tx.update_mint_quote(&mut quote)
        .await
        .expect("persist altered quote");
    tx.commit().await.expect("commit altered quote");

    let error = state
        .federation
        .audit_trusted_checkpoint_materialized_state()
        .await
        .expect_err("operator audit must reject altered state");
    assert!(matches!(
        error,
        cdk::Error::Federation(FederationError::MaterializedStateAuditMismatch { .. })
    ));
    assert!(
        db.get_federation_checkpoint_frontier()
            .await
            .expect("checkpoint frontier")
            .materialized_state_audit_mismatch
    );
    let restart_error = state
        .federation
        .audit_trusted_checkpoint_materialized_state()
        .await
        .expect_err("latched mismatch must remain fail closed");
    assert!(matches!(
        restart_error,
        cdk::Error::Federation(FederationError::CheckpointInvalid { .. })
    ));
}

#[tokio::test]
async fn checkpoint_catch_up_applies_bounded_history_and_promotes_target() {
    let (public_config, source_private, target_private) = transport_config();
    let seed = [57_u8; 64];
    let source_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("source db"));
    let target_db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("target db"));
    let source = federation_route_state_for_private_with_db(
        public_config.clone(),
        source_private.clone(),
        Arc::clone(&source_db),
        &seed,
    )
    .await;
    let mut target = federation_route_state_for_private_with_db(
        public_config.clone(),
        target_private.clone(),
        Arc::clone(&target_db),
        &seed,
    )
    .await;
    let quote_id = QuoteId::new();
    source
        .submit_federation_envelope(mint_quote_envelope(&source, &quote_id))
        .await
        .expect("source accepted checkpointed operation");

    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &source,
        &public_config,
        None,
        &[&source_private, &target_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("checkpoint id");
    let mut target_tx = target_db
        .begin_transaction()
        .await
        .expect("begin target checkpoint transaction");
    target_tx
        .add_federation_checkpoint_quorum(&quorum)
        .await
        .expect("persist target checkpoint proof");
    target_tx
        .commit()
        .await
        .expect("commit target checkpoint proof");
    target.mark_federation_awaiting_quorum(0).await;
    let restart_client = FederationRestPeerClient::new(
        MockFederationTransport::default(),
        &public_config,
        target_private.clone(),
    )
    .expect("restart checkpoint client");
    assert_eq!(
        sync_loop::run_federation_peer_sync_round(&target, &restart_client, &[]).await,
        0
    );
    assert!(target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("ahead checkpoint remains untrusted")
        .is_none());
    let ahead_frontier = target_db
        .get_federation_checkpoint_frontier()
        .await
        .expect("frontier before checkpoint suffix");
    assert_eq!(ahead_frontier.journal_next_index, 0);
    assert_eq!(ahead_frontier.application_next_index, 0);

    let source_entries = source_db
        .get_federation_journal_entries()
        .await
        .expect("source journal");
    let source_journal =
        InMemoryFederationJournal::from_entries(source_entries).expect("source journal");
    let finalized_items = source_db
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("source finalized items");
    let request = FederationJournalCatchUpRequest::new(0)
        .with_target_checkpoint(checkpoint_id)
        .with_aleph_bft_next_item_index(0);
    let response = FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        &source_journal,
        checkpoint.materialized_state_digest,
        &request,
        100,
        100,
    )
    .bound_to_checkpoint(&checkpoint)
    .expect("checkpoint-bounded response")
    .with_aleph_bft_finalized_items(finalized_items, true);
    response
        .validate_checkpoint_binding(&checkpoint)
        .expect("checkpoint response binding");

    assert!(target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("target trusted checkpoint before catch-up")
        .is_none());
    let mut bad_signature_quorum = quorum.clone();
    let original_signature = bad_signature_quorum.signatures[0];
    let mut bad_signature_bytes = original_signature.to_bytes();
    bad_signature_bytes[0] ^= 1;
    bad_signature_quorum.signatures[0] = FederationCheckpointSignature::from_slice(
        original_signature.checkpoint_id,
        original_signature.signer,
        &bad_signature_bytes,
    )
    .expect("bad checkpoint signature fixture");
    target
        .apply_federation_checkpoint_catch_up_response(&response, &bad_signature_quorum)
        .await
        .expect_err("bad checkpoint signature must fail before application");
    let minority_quorum =
        FederationCheckpointQuorum::new(checkpoint.clone(), vec![quorum.signatures[0]]);
    target
        .apply_federation_checkpoint_catch_up_response(&response, &minority_quorum)
        .await
        .expect_err("minority checkpoint quorum must fail before application");
    let mut tampered = response.clone();
    tampered.entries[0].operation_id = FederationOperationId::from_bytes([99; 32]);
    target
        .apply_federation_checkpoint_catch_up_response(&tampered, &quorum)
        .await
        .expect_err("tampered checkpoint batch must fail before application");
    assert_eq!(
        target_db
            .get_federation_checkpoint_frontier()
            .await
            .expect("target frontier after rejected batch")
            .journal_next_index,
        0
    );
    assert!(target_db
        .get_mint_quote(&quote_id)
        .await
        .expect("target quote after rejected batch")
        .is_none());
    target.federation.fail_next_checkpoint_promotion();
    target
        .apply_federation_checkpoint_catch_up_response(&response, &quorum)
        .await
        .expect_err("injected pre-promotion failure");
    assert!(target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint after pre-promotion failure")
        .is_none());
    let staged_frontier = target_db
        .get_federation_checkpoint_frontier()
        .await
        .expect("staged frontier before promotion restart");
    assert!(staged_frontier.exactly_matches_checkpoint(&checkpoint));

    let (restarted_consensus, restarted_sequences) =
        crate::create_federation_runtime_state(&target.mint)
            .await
            .expect("restart exact staged checkpoint frontier");
    target.federation.consensus = restarted_consensus;
    target.federation.sequences = restarted_sequences;
    target
        .mark_federation_awaiting_quorum(checkpoint.journal_next_index)
        .await;
    let resumed_target = sync_loop::next_local_checkpoint_catch_up_target(&target)
        .await
        .expect("resume staged checkpoint target")
        .expect("staged checkpoint remains a target until promotion");
    assert_eq!(
        resumed_target
            .checkpoint
            .id()
            .expect("resumed checkpoint id"),
        checkpoint_id
    );
    assert_eq!(
        sync_loop::run_federation_peer_sync_round(&target, &restart_client, &[]).await,
        0
    );
    assert!(target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint after resumed sync round")
        .is_some());
    assert!(sync_loop::next_local_checkpoint_catch_up_target(&target)
        .await
        .expect("inspect promoted checkpoint target")
        .is_none());

    let trusted = target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("target trusted checkpoint after catch-up")
        .expect("promoted target checkpoint");
    assert_eq!(
        trusted.checkpoint.id().expect("trusted checkpoint id"),
        checkpoint_id
    );
    let target_frontier = target_db
        .get_federation_checkpoint_frontier()
        .await
        .expect("target checkpoint frontier");
    assert_eq!(
        target_frontier.journal_next_index,
        checkpoint.journal_next_index
    );
    assert_eq!(
        target_frontier.application_digest,
        checkpoint.application_digest
    );
    assert_eq!(
        target_frontier.finalized_item_digest,
        checkpoint.finalized_item_digest
    );
    assert_eq!(
        target
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("target materialized state")
            .digest()
            .expect("target materialized-state digest"),
        checkpoint.materialized_state_digest
    );
    assert!(target_db
        .get_mint_quote(&quote_id)
        .await
        .expect("target quote lookup")
        .is_some());

    let duplicate_payload = {
        let consensus = source
            .federation
            .consensus
            .as_ref()
            .expect("source consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::operation(mint_quote_envelope(
                &source, &quote_id,
            ))],
        )
        .expect("post-checkpoint duplicate payload")
    };
    let duplicate = source
        .federation
        .apply_finalized_unit(duplicate_payload)
        .await
        .expect("finalize post-checkpoint duplicate")
        .pop()
        .expect("duplicate response");
    assert!(matches!(
        duplicate.outcome,
        FederationJournalAppendOutcome::Duplicate(ref entry) if entry.index == 0
    ));

    let second_quote_id = QuoteId::new();
    source
        .submit_federation_envelope(mint_quote_envelope(&source, &second_quote_id))
        .await
        .expect("source accepted checkpoint suffix operation");
    let (second_checkpoint, second_quorum) = recovery_checkpoint_quorum_for_state(
        &source,
        &public_config,
        None,
        &[&source_private, &target_private],
    )
    .await;
    let second_checkpoint_id = second_checkpoint.id().expect("second checkpoint id");
    let mut target_tx = target_db
        .begin_transaction()
        .await
        .expect("begin second checkpoint transaction");
    target_tx
        .add_federation_checkpoint_quorum(&second_quorum)
        .await
        .expect("persist second checkpoint proof");
    target_tx
        .commit()
        .await
        .expect("commit second checkpoint proof");
    let source_entries = source_db
        .get_federation_journal_entries()
        .await
        .expect("source journal with suffix");
    let source_journal =
        InMemoryFederationJournal::from_entries(source_entries).expect("source suffix journal");
    let finalized_items = source_db
        .get_federation_aleph_bft_finalized_items_from(checkpoint.finalized_next_item_index)
        .await
        .expect("source finalized suffix");
    assert_eq!(finalized_items.len(), 2);
    let request = FederationJournalCatchUpRequest::new(checkpoint.journal_next_index)
        .with_target_checkpoint(second_checkpoint_id)
        .with_aleph_bft_next_item_index(checkpoint.finalized_next_item_index);
    let response = FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        &source_journal,
        second_checkpoint.materialized_state_digest,
        &request,
        100,
        100,
    )
    .bound_to_checkpoint(&second_checkpoint)
    .expect("second checkpoint-bounded response")
    .with_aleph_bft_finalized_items(finalized_items, true);
    let second_applied = target
        .apply_federation_checkpoint_catch_up_response(&response, &second_quorum)
        .await
        .expect("apply checkpoint suffix");

    assert_eq!(second_applied.len(), 1);
    assert_eq!(second_applied[0].index, checkpoint.journal_next_index);
    let target_consensus = target
        .federation
        .consensus
        .as_ref()
        .expect("target consensus")
        .lock()
        .await;
    assert_eq!(target_consensus.status().accepted_operation_count, 2);
    assert!(target_consensus.journal().entries().is_empty());
    assert!(target_consensus.finalized_items().is_empty());
    drop(target_consensus);
    assert_eq!(
        target_db
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("second trusted checkpoint")
            .expect("promoted second checkpoint")
            .checkpoint
            .id()
            .expect("second trusted checkpoint id"),
        second_checkpoint_id
    );
    assert!(target_db
        .get_mint_quote(&second_quote_id)
        .await
        .expect("target suffix quote lookup")
        .is_some());
    eprintln!(
        "checkpoint_catch_up_baseline targets=2 first_journal_items=1 first_finalized_items=1 second_journal_items=1 second_finalized_items=2 promotions=2"
    );
}

#[tokio::test]
async fn empty_database_bootstrap_crosses_scheduled_checkpoints_and_recovery() {
    let (public_config, source_private, target_private) = transport_config();
    let source =
        federation_route_state_for_private(public_config.clone(), source_private.clone()).await;
    let target =
        federation_route_state_for_private(public_config.clone(), target_private.clone()).await;
    let signers = [&source_private, &target_private];

    source
        .submit_federation_envelope(mint_quote_envelope(&source, &QuoteId::new()))
        .await
        .expect("first scheduled operation");
    let mut first_scheduled = recovery_checkpoint_for_state(&source, &public_config, None).await;
    first_scheduled.kind = FederationCheckpointKind::Scheduled;
    first_scheduled.scheduled_checkpoint_sequence = 1;
    let first_signatures = signers
        .iter()
        .map(|signer| {
            FederationCheckpointSignature::new_signed_no_aux_rand(&first_scheduled, signer)
                .expect("first scheduled signature")
        })
        .collect();
    let first_quorum = FederationCheckpointQuorum::new(first_scheduled.clone(), first_signatures);
    first_quorum
        .verify(&public_config)
        .expect("first scheduled quorum");

    source
        .submit_federation_envelope(mint_quote_envelope(&source, &QuoteId::new()))
        .await
        .expect("second scheduled operation");
    let mut second_scheduled =
        recovery_checkpoint_for_state(&source, &public_config, Some(&first_scheduled)).await;
    second_scheduled.kind = FederationCheckpointKind::Scheduled;
    second_scheduled.scheduled_checkpoint_sequence = 2;
    second_scheduled
        .validate_promotion_from(Some(&first_scheduled))
        .expect("second scheduled lineage");
    let second_signatures = signers
        .iter()
        .map(|signer| {
            FederationCheckpointSignature::new_signed_no_aux_rand(&second_scheduled, signer)
                .expect("second scheduled signature")
        })
        .collect();
    let second_quorum =
        FederationCheckpointQuorum::new(second_scheduled.clone(), second_signatures);
    second_quorum
        .verify(&public_config)
        .expect("second scheduled quorum");

    source
        .submit_federation_envelope(mint_quote_envelope(&source, &QuoteId::new()))
        .await
        .expect("recovery operation");
    let (recovery, recovery_quorum) = recovery_checkpoint_quorum_for_state(
        &source,
        &public_config,
        Some(&second_scheduled),
        &signers,
    )
    .await;

    let source_entries = source
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("source journal");
    let source_journal =
        InMemoryFederationJournal::from_entries(source_entries).expect("source journal chain");
    let source_finalized = source
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("source finalized items");
    let checkpoints = [
        (&first_scheduled, &first_quorum),
        (&second_scheduled, &second_quorum),
        (&recovery, &recovery_quorum),
    ];
    let mut journal_from = 0_u64;
    let mut finalized_from = 0_u64;
    for (checkpoint, quorum) in checkpoints {
        let checkpoint_id = checkpoint.id().expect("checkpoint id");
        let request = FederationJournalCatchUpRequest::new(journal_from)
            .with_target_checkpoint(checkpoint_id)
            .with_aleph_bft_next_item_index(finalized_from);
        let finalized_start = usize::try_from(finalized_from).expect("finalized start");
        let finalized_end =
            usize::try_from(checkpoint.finalized_next_item_index).expect("finalized end");
        let response = FederationJournalCatchUpResponse::from_journal(
            public_config.federation_id,
            &source_journal,
            checkpoint.materialized_state_digest,
            &request,
            1,
            1,
        )
        .bound_to_checkpoint(checkpoint)
        .expect("checkpoint-bound bootstrap page")
        .with_aleph_bft_finalized_items(
            source_finalized[finalized_start..finalized_end].to_vec(),
            true,
        );
        target
            .apply_federation_checkpoint_catch_up_response(&response, quorum)
            .await
            .expect("apply bootstrap checkpoint");
        let consensus = target
            .federation
            .consensus
            .as_ref()
            .expect("target consensus")
            .lock()
            .await;
        assert!(consensus.journal().entries().is_empty());
        assert!(consensus.finalized_items().is_empty());
        drop(consensus);
        journal_from = checkpoint.journal_next_index;
        finalized_from = checkpoint.finalized_next_item_index;
    }

    assert_eq!(journal_from, 3);
    assert_eq!(finalized_from, 3);
    assert_eq!(
        target
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted checkpoint")
            .expect("trusted recovery checkpoint")
            .checkpoint
            .id()
            .expect("trusted checkpoint id"),
        recovery.id().expect("recovery id")
    );
}

#[tokio::test]
async fn runtime_components_complete_concurrent_checkpoint_and_finality_work_without_deadlock() {
    let (state, public_config, peer_private) = federation_route_state().await;
    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("apply component concurrency checkpoint base");
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        None,
        &[&local_private, &peer_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("component checkpoint id");
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin component checkpoint candidate transaction");
    tx.add_federation_checkpoint_candidate(&checkpoint)
        .await
        .expect("store component checkpoint candidate");
    tx.commit()
        .await
        .expect("commit component checkpoint candidate");
    let empty_response = FederationJournalCatchUpResponse::from_journal(
        checkpoint.federation_id,
        &InMemoryFederationJournal::from_base(
            checkpoint.journal_next_index,
            checkpoint.journal_order_digest,
        ),
        checkpoint.materialized_state_digest,
        &FederationJournalCatchUpRequest::new(checkpoint.journal_next_index)
            .with_target_checkpoint(checkpoint_id)
            .with_aleph_bft_next_item_index(checkpoint.finalized_next_item_index),
        1,
        1,
    )
    .bound_to_checkpoint(&checkpoint)
    .expect("bind empty component checkpoint response")
    .with_aleph_bft_finalized_items(Vec::new(), true);

    let start = Arc::new(tokio::sync::Barrier::new(6));
    let finality = {
        let state = state.clone();
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            state
                .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
                .await
        })
    };
    let catch_up = {
        let state = state.clone();
        let quorum = quorum.clone();
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            state
                .apply_federation_checkpoint_catch_up_response(&empty_response, &quorum)
                .await
        })
    };
    let quorum_exchange = {
        let state = state.clone();
        let quorum = quorum.clone();
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            state.federation.persist_checkpoint_quorum(&quorum).await
        })
    };
    let signature_exchange = {
        let state = state.clone();
        let signature = quorum.signatures[0];
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            state
                .federation
                .persist_checkpoint_signature(checkpoint_id, &signature)
                .await
        })
    };
    let status = {
        let state = state.clone();
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            state.federation.checkpoint_summary(None).await
        })
    };
    let shutdown = {
        let state = state.clone();
        tokio::spawn(async move {
            start.wait().await;
            super::super::halt_current_federation_aleph_bft_session(
                &state,
                "runtime component concurrency test shutdown",
            )
            .await;
        })
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        let (finality, catch_up, quorum_exchange, signature_exchange, status, shutdown) =
            tokio::join!(
                finality,
                catch_up,
                quorum_exchange,
                signature_exchange,
                status,
                shutdown
            );
        let _ = finality.expect("finality task completed");
        let _ = catch_up.expect("catch-up task completed");
        let _ = quorum_exchange.expect("quorum exchange task completed");
        let _ = signature_exchange.expect("signature exchange task completed");
        let _ = status.expect("status task completed");
        shutdown.expect("shutdown task completed");
    })
    .await
    .expect("concurrent runtime component work completed without deadlock");
}

#[tokio::test]
async fn checkpoint_peer_request_does_not_hold_runtime_mutation_locks() {
    let (public_config, target_private, source_private) = transport_config();
    let source =
        federation_route_state_for_private(public_config.clone(), source_private.clone()).await;
    let target =
        federation_route_state_for_private(public_config.clone(), target_private.clone()).await;
    source
        .submit_federation_envelope(mint_quote_envelope(&source, &QuoteId::new()))
        .await
        .expect("source accepted lock-boundary operation");
    let (_, quorum) = recovery_checkpoint_quorum_for_state(
        &source,
        &public_config,
        None,
        &[&target_private, &source_private],
    )
    .await;
    let source_member = public_config
        .member(source_private.member_id)
        .expect("source member")
        .clone();
    let request_started = Arc::new(tokio::sync::Notify::new());
    let client = FederationRestPeerClient::new(
        DelayTransport {
            delay: Duration::from_secs(30),
            request_started: Some(Arc::clone(&request_started)),
        },
        &public_config,
        target_private,
    )
    .expect("delayed checkpoint client");

    let catch_up_target = target.clone();
    let catch_up = tokio::spawn(async move {
        client
            .catch_up_state_to_checkpoint_from_member(&catch_up_target, &source_member, &quorum)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), request_started.notified())
        .await
        .expect("checkpoint peer request started");
    assert!(
        !catch_up.is_finished(),
        "checkpoint catch-up should still be waiting on the peer"
    );

    let application = target.federation.operation_application_lock_for_test();
    let application_guard = tokio::time::timeout(Duration::from_secs(1), application.lock())
        .await
        .expect("peer request left the application lock available");
    let consensus = target
        .federation
        .consensus
        .as_ref()
        .expect("target consensus");
    let consensus_guard = tokio::time::timeout(Duration::from_secs(1), consensus.lock())
        .await
        .expect("peer request left the consensus lock available");
    drop(consensus_guard);
    drop(application_guard);

    catch_up.abort();
    let cancellation = catch_up
        .await
        .expect_err("blocked checkpoint request was cancelled");
    assert!(cancellation.is_cancelled());
}

#[tokio::test]
async fn long_offline_member_recovers_from_one_quorum_holder_without_signer_work() {
    let federation_api_urls = (1u16..=7)
        .map(|value| {
            MintUrl::from_str(&format!("https://checkpoint-peer-{value}.test"))
                .expect("federation api url")
        })
        .collect();
    let (public_config, private_configs) = transport_config_with_member_federation_api_urls(
        ThresholdParams::with_bft_consensus(7, 4).expect("threshold params"),
        federation_api_urls,
    );
    let source_private = private_configs[5].clone();
    let target_private = private_configs[6].clone();
    let source =
        federation_route_state_for_private(public_config.clone(), source_private.clone()).await;
    let target =
        federation_route_state_for_private(public_config.clone(), target_private.clone()).await;
    let quote_ids = (0..3).map(|_| QuoteId::new()).collect::<Vec<_>>();
    for quote_id in &quote_ids {
        source
            .submit_federation_envelope(mint_quote_envelope(&source, quote_id))
            .await
            .expect("source accepted offline-history operation");
    }

    let signers = private_configs.iter().take(5).collect::<Vec<_>>();
    let (checkpoint, quorum) =
        recovery_checkpoint_quorum_for_state(&source, &public_config, None, &signers).await;
    let checkpoint_id = checkpoint.id().expect("offline recovery checkpoint id");
    let source_db = source.mint.localstore();
    let mut tx = source_db
        .begin_transaction()
        .await
        .expect("begin source checkpoint promotion");
    tx.promote_verified_federation_checkpoint(
        &quorum
            .verify(&public_config)
            .expect("verify source checkpoint quorum"),
    )
    .await
    .expect("promote source checkpoint");
    tx.commit().await.expect("commit source checkpoint");
    seed_synthetic_active_session_backup(&source, checkpoint.aleph_bft_session, 1, 16).await;

    let transport = RouterFederationTransportRegistry::default();
    let signer_requests = Arc::new(AtomicUsize::new(0));
    for signer in &signers {
        let requests = Arc::clone(&signer_requests);
        transport.insert(
            public_config
                .member(signer.member_id)
                .expect("historical signer")
                .federation_api_url
                .clone(),
            Router::new().fallback(move || {
                let requests = Arc::clone(&requests);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    StatusCode::SERVICE_UNAVAILABLE
                }
            }),
        );
    }
    let router = Router::new()
        .nest("/federation/v1", create_federation_router())
        .with_state(source);
    transport.insert(
        public_config
            .member(source_private.member_id)
            .expect("quorum-holder member")
            .federation_api_url
            .clone(),
        router,
    );
    let client = FederationRestPeerClient::new(transport, &public_config, target_private)
        .expect("single-source checkpoint client");
    let source_member = public_config
        .member(source_private.member_id)
        .expect("quorum-holder member");

    client
        .sync_once_from_member(&target, source_member)
        .await
        .expect("discover offline checkpoint target");
    assert_eq!(
        exchange_checkpoint_round(&target, &client, &[source_member]).await,
        1
    );
    let target_db = target.mint.localstore();
    assert!(target_db
        .get_federation_checkpoint_quorum(checkpoint_id)
        .await
        .expect("fetched checkpoint quorum")
        .is_some());
    assert!(target_db
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("unpromoted fetched checkpoint")
        .is_none());

    let peer_status = client
        .sync_once_from_member(&target, source_member)
        .await
        .expect("catch up offline member to checkpoint");
    assert_eq!(peer_status.operation_count, 3);
    assert_eq!(
        target_db
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted recovered checkpoint")
            .expect("promoted recovered checkpoint")
            .checkpoint
            .id()
            .expect("trusted recovered checkpoint id"),
        checkpoint_id
    );
    for quote_id in &quote_ids {
        assert!(target_db
            .get_mint_quote(quote_id)
            .await
            .expect("recovered quote lookup")
            .is_some());
    }
    let consensus = target
        .federation
        .consensus
        .as_ref()
        .expect("recovered target consensus")
        .lock()
        .await;
    assert_eq!(consensus.status().accepted_operation_count, 3);
    assert!(consensus.journal().entries().is_empty());
    assert!(consensus.finalized_items().is_empty());
    drop(consensus);
    assert_eq!(
        signer_requests.load(Ordering::SeqCst),
        0,
        "offline recovery contacted historical signers instead of using the stored quorum"
    );
}

#[tokio::test]
async fn federation_dkg_setup_runtime_has_no_persisted_application_consensus() {
    let (mut public_config, mut local_private, _) = transport_config();
    public_config.keysets.clear();
    public_config.setup_transcript = None;
    local_private.keyset_secret_shares.clear();
    local_private.setup_transcript_hash = None;
    public_config.validate().expect("DKG setup public config");
    local_private
        .validate_against_public(&public_config)
        .expect("DKG setup private config");
    let state = federation_route_state_for_private(public_config.clone(), local_private).await;

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("DKG setup runtime state");

    assert!(consensus.is_some());
    assert!(sequences.is_some());
    let snapshot = state
        .mint
        .localstore()
        .get_federation_config(public_config.federation_id)
        .await
        .expect("load setup federation config")
        .expect("setup federation config snapshot");
    assert_eq!(snapshot.config, public_config);
    assert!(state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("load pre-finalization consensus units")
        .is_empty());
}

#[tokio::test]
async fn federation_runtime_startup_rejects_conflicting_public_config_snapshot() {
    let (public_config, local_private, _) = transport_config();
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));

    let mut conflicting_config = public_config.clone();
    conflicting_config.payment_observation_policy.mint_quotes =
        FederationPaymentVerificationPolicy::member_quorum(
            conflicting_config.threshold_params.consensus_threshold,
        );
    conflicting_config
        .validate()
        .expect("conflicting config validates");
    let snapshot =
        FederationConfigSnapshot::new(conflicting_config).expect("conflicting config snapshot");
    assert_eq!(snapshot.federation_id, public_config.federation_id);
    assert_ne!(
        snapshot.config_digest,
        public_config.config_digest().expect("public config digest")
    );

    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin conflicting config transaction");
    tx.set_federation_config(&snapshot)
        .await
        .expect("persist conflicting config");
    tx.commit().await.expect("commit conflicting config");

    let mnemonic = Mnemonic::generate(12).expect("mnemonic");
    let state = federation_route_state_for_private_with_db(
        public_config,
        local_private,
        db,
        &mnemonic.to_seed_normalized(""),
    )
    .await;
    let err = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect_err("conflicting config rejected");
    assert!(
        err.to_string().contains("Conflicting federation config"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn federation_runtime_state_replays_persisted_journal_and_dkg_sequences() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let peer_member = peer_private.member_id;
    let envelope = test_operation_envelope_for_config(
        &public_config,
        "0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41",
    );
    let response = state
        .submit_federation_envelope(envelope)
        .await
        .expect("persist operation");
    state
        .persist_federation_dkg_sequence(peer_member, 42)
        .await
        .expect("persist DKG sequence");

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("runtime state");
    let consensus = consensus.expect("consensus");
    let sequences = sequences.expect("sequences");
    let consensus = consensus.lock().await;
    let sequences = sequences.lock().await;

    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .journal()
            .entries()
            .first()
            .expect("journal entry")
            .operation_id,
        response.operation_id
    );
    assert_eq!(sequences.dkg_sequences().get(&peer_member), Some(&42));
}

#[tokio::test]
async fn federation_runtime_startup_skips_terminal_rejected_entries() {
    let (state, _, _) = federation_route_state().await;
    let quote_id = QuoteId::new();
    let envelope = mint_quote_envelope(&state, &quote_id);
    let mut conflict = envelope.clone();
    let FederationOperation::MintQuote(operation) = &mut conflict.operation else {
        panic!("mint quote operation");
    };
    let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response else {
        panic!("bolt11 mint quote response");
    };
    response.request.push_str("-conflict");
    let conflict = conflict
        .authorize(
            state
                .mint
                .federation_config()
                .expect("federation config")
                .private_config(),
        )
        .expect("authorize conflicting operation");
    let adapter = finalized_aleph_bft_history(
        state
            .mint
            .federation_config()
            .expect("federation config")
            .public_config(),
        member_id(1),
        [envelope, conflict],
    );
    let accepted_entry = adapter.journal().entries()[0].clone();
    let rejected_entry = adapter.journal().entries()[1].clone();
    assert_eq!(accepted_entry.status(), FederationOperationStatus::Accepted);
    assert_eq!(rejected_entry.status(), FederationOperationStatus::Rejected);

    persist_finalized_aleph_bft_history(&state, &adapter).await;

    let (consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("terminal rejected entry must not block startup");
    let consensus = consensus.expect("consensus");
    let consensus = consensus.lock().await;
    let recovered = consensus
        .journal()
        .entry_by_operation_id(rejected_entry.operation_id)
        .expect("recovered rejected entry");
    assert_eq!(recovered.status(), FederationOperationStatus::Rejected);
    assert_eq!(consensus.peer_status().operation_count, 2);
    drop(consensus);
    let quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup")
        .expect("accepted quote applied during startup");
    assert_eq!(quote.request, format!("invoice-{quote_id}"));
}

#[tokio::test]
async fn federation_runtime_startup_reloads_aleph_bft_finalized_items_fail_closed() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
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
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    adapter.finalize_unit(unit).expect("finalize unit");
    let entry = adapter.journal().entries()[0].clone();
    let finalized_items = adapter.finalized_items().to_vec();

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT startup transaction");
    tx.add_federation_aleph_bft_finalized_items(&finalized_items)
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist matching journal entry");
    tx.commit().await.expect("commit AlephBFT startup state");

    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before startup replay")
        .is_none());

    let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("AlephBFT runtime state");
    let consensus = consensus.expect("consensus");
    assert!(sequences.is_some());

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after startup replay")
        .expect("mint quote replayed from finalized items");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));

    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus.journal().entries()[0].operation_id,
        entry.operation_id
    );
    assert_eq!(
        consensus.journal().entries()[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn router_startup_replays_aleph_bft_finalized_items_before_serving_local_status() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
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
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("AlephBFT finalized unit");
    adapter.finalize_unit(unit).expect("finalize unit");
    let entry = adapter.journal().entries()[0].clone();

    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin AlephBFT router startup transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized items");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist matching journal entry");
    tx.commit()
        .await
        .expect("commit AlephBFT router startup state");
    assert!(state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup before router startup")
        .is_none());

    let router =
        crate::create_mint_router(state.mint.clone(), vec![KnownMethod::Bolt11.to_string()])
            .await
            .expect("mint router");
    let request = signed_status_request_with_payload(
        &public_config,
        &peer_private,
        local_private.member_id,
        1,
        FederationPeerStatusRequest::with_materialized_rows(),
    );
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method(axum::http::Method::POST)
                .uri("/federation/v1/local-status")
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&request).expect("request json"),
                ))
                .expect("local status request"),
        )
        .await
        .expect("local status response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("local status body");
    let local_status: FederationLocalStatusResponse =
        serde_json::from_slice(&body).expect("local status json");

    assert_eq!(local_status.peer_status.operation_count, 1);
    assert!(!local_status.sync_ready);
    assert_eq!(local_status.sync_state, "awaiting_quorum");
    assert!(
        local_status
            .sync_message
            .contains("awaiting a quorum-compatible peer view"),
        "unexpected sync message: {}",
        local_status.sync_message
    );
    assert!(!local_status.aleph_bft_live_session_active);
    assert!(!local_status.aleph_bft_network_inbound_active);
    let aleph_bft_status = local_status
        .aleph_bft_status
        .expect("AlephBFT local status");
    assert_eq!(aleph_bft_status.accepted_operation_count, 1);
    assert!(
        local_status
            .materialized_state_rows
            .iter()
            .any(|row| row.table == "mint_quotes"),
        "router startup should materialize AlephBFT finalized mint quote before serving status"
    );

    let replayed_quote = state
        .mint
        .localstore()
        .get_mint_quote(&quote_id)
        .await
        .expect("quote lookup after router startup")
        .expect("mint quote replayed from finalized items");
    assert_eq!(replayed_quote.request, format!("invoice-{quote_id}"));
    let replayed_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("replayed journal entries");
    assert_eq!(replayed_entries.len(), 1);
    assert_eq!(replayed_entries[0].operation_id, operation_id);
    assert_eq!(
        replayed_entries[0].status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn finalized_catch_up_does_not_carry_backup_state() {
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
    let envelope = mint_quote_envelope(&state, &quote_id);
    let operation_id = envelope.operation_id().expect("operation id");
    let first_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(envelope.clone())],
    )
    .expect("first AlephBFT unit");

    state
        .apply_federation_aleph_bft_finalized_unit(first_unit.clone())
        .await
        .expect("local first finalized unit");
    let mut peer_adapter = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        peer_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("peer AlephBFT adapter");
    peer_adapter
        .finalize_unit(first_unit)
        .expect("peer first finalized unit");
    let duplicate_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        peer_private.member_id,
        0,
        1,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("duplicate AlephBFT unit");
    peer_adapter
        .finalize_unit(duplicate_unit)
        .expect("peer duplicate finalized unit");
    assert_eq!(peer_adapter.journal().entries().len(), 1);
    assert_eq!(peer_adapter.finalized_items().len(), 2);

    let request = state
        .federation
        .consensus
        .as_ref()
        .expect("local consensus")
        .lock()
        .await
        .catch_up_request();
    assert_eq!(request.next_index, 1);
    let mut response = cdk::federation::FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        peer_adapter.journal(),
        FederationMaterializedStateDigest::genesis(),
        &request,
        50,
        100,
    );
    response.materialized_state_digest =
        materialized_state_digest(&state, peer_adapter.journal().entries())
            .await
            .expect("materialized state digest");
    let response =
        response.with_aleph_bft_finalized_items(peer_adapter.finalized_items()[1..].to_vec(), true);
    state
        .apply_federation_catch_up_response(&response)
        .await
        .expect("apply finalized-item catch-up independently");
    assert!(serde_json::to_value(&response)
        .expect("catch-up response JSON")
        .get("aleph_bft_backup")
        .is_none());
    let stored_finalized_items = state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("stored finalized items");
    assert_eq!(stored_finalized_items.len(), 2);
    assert_eq!(stored_finalized_items[1].operation_id, Some(operation_id));
    let stored_entries = state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("stored journal entries");
    assert_eq!(stored_entries.len(), 1);
    let consensus = state.federation.consensus.as_ref().expect("consensus");
    let consensus = consensus.lock().await;
    assert_eq!(consensus.peer_status().operation_count, 1);
    assert_eq!(
        consensus
            .aleph_bft_status()
            .expect("AlephBFT status")
            .next_item_index,
        2
    );
}

#[tokio::test]
async fn checkpoint_page_streaming_keeps_only_one_bounded_page_in_flight() {
    let (state, public_config, local_private, _, _) =
        federation_route_state_three_with_local_private().await;
    let mut runtime = FederationConsensusRuntime::for_members(
        public_config.federation_id,
        local_private.member_id,
        0,
        public_config.members.keys().copied(),
    )
    .expect("checkpoint aggregation runtime");
    let operation_count = 12_usize;
    for unit_index in 0..operation_count as u64 {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            local_private.member_id,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(mint_quote_envelope(
                &state,
                &QuoteId::new(),
            ))],
        )
        .expect("checkpoint aggregation unit");
        runtime
            .finalize_unit(unit)
            .expect("finalize checkpoint aggregation unit");
    }

    let target_checkpoint_id = cdk::federation::FederationCheckpointId::from_bytes([71; 32]);
    let mut next_index = 0_usize;
    let mut page_count = 0_usize;
    let mut largest_page_bytes = 0_usize;
    let mut transferred_bytes = 0_usize;
    while next_index < operation_count {
        let request = FederationJournalCatchUpRequest::new(next_index as u64)
            .with_target_checkpoint(target_checkpoint_id)
            .with_limit(3)
            .with_aleph_bft_next_item_index(next_index as u64);
        let page = FederationJournalCatchUpResponse::from_journal(
            public_config.federation_id,
            runtime.journal(),
            cdk::federation::FederationMaterializedStateDigest::genesis(),
            &request,
            3,
            3,
        );
        let page_end = next_index.saturating_add(page.entries.len());
        let page = page.with_aleph_bft_finalized_items(
            runtime.finalized_items()[next_index..page_end].to_vec(),
            page_end == operation_count,
        );
        let page_bytes = page
            .ensure_within_checkpoint_page_limit()
            .expect("bounded checkpoint streaming page");
        largest_page_bytes = largest_page_bytes.max(page_bytes);
        transferred_bytes = transferred_bytes.saturating_add(page_bytes);
        next_index = page_end;
        page_count = page_count.saturating_add(1);
    }

    assert_eq!(page_count, 4);
    assert!(
        transferred_bytes > largest_page_bytes.saturating_mul(3),
        "the transfer must span several independently bounded pages"
    );
    eprintln!(
        "checkpoint_catch_up_streaming pages={page_count} operations={operation_count} largest_page_bytes={largest_page_bytes} transferred_bytes={transferred_bytes}"
    );
}

#[tokio::test]
async fn scheduled_checkpoint_lineage_does_not_depend_on_quorum_arrival() {
    let (mut public, first_private, second_private) = transport_config();
    public.checkpoint_policy =
        FederationCheckpointPolicy::new(1, 100).expect("checkpoint each journal operation");
    let first = federation_route_state_for_private(public.clone(), first_private.clone()).await;
    let second = federation_route_state_for_private(public.clone(), second_private.clone()).await;
    let first_db = first.mint.localstore();
    let second_db = second.mint.localstore();
    let mut checkpoints = Vec::new();
    for sequence in 1..=3 {
        let items = vec![FederationConsensusItem::operation(mint_quote_envelope(
            &first,
            &QuoteId::new(),
        ))];
        apply_identical_finalized_unit(&first, &second, items).await;
        let left = first_db
            .get_next_federation_scheduled_checkpoint(sequence - 1)
            .await
            .expect("first candidate")
            .expect("first stored candidate");
        let right = second_db
            .get_next_federation_scheduled_checkpoint(sequence - 1)
            .await
            .expect("second candidate")
            .expect("second stored candidate");
        assert_eq!(left, right, "signature delivery cannot select the boundary");
        assert_eq!(left.scheduled_checkpoint_sequence, sequence);
        let id = left.id().expect("candidate id");
        let mut signatures = first_db
            .get_federation_checkpoint_signatures(id)
            .await
            .expect("first stored signature");
        signatures.extend(
            second_db
                .get_federation_checkpoint_signatures(id)
                .await
                .expect("second stored signature"),
        );
        let quorum = FederationCheckpointQuorum::new(left.clone(), signatures);
        quorum
            .verify(&public)
            .expect("matching independently captured state");
        if sequence == 1 {
            // The second member learns the first quorum only after more units
            // have finalized. It must still capture the same next candidates.
            first
                .federation
                .persist_checkpoint_quorum(&quorum)
                .await
                .expect("early quorum");
        }
        checkpoints.push(quorum);
    }
    for quorum in &checkpoints {
        let id = quorum.checkpoint.id().expect("checkpoint id");
        for db in [&first_db, &second_db] {
            assert_eq!(
                db.get_federation_checkpoint(id)
                    .await
                    .expect("retained ancestor"),
                Some(quorum.checkpoint.clone())
            );
        }
        first
            .federation
            .persist_checkpoint_quorum(quorum)
            .await
            .expect("first ordered promotion");
    }
    // Quorums can be received out of order before their ancestors are promoted.
    // Healthy status and equal peer operation counts must not strand local trust.
    let mut tx = second_db
        .begin_transaction()
        .await
        .expect("delayed delivery");
    for quorum in checkpoints.iter().rev() {
        tx.add_federation_checkpoint_quorum(quorum)
            .await
            .expect("persist delayed quorum without promotion");
    }
    tx.commit().await.expect("commit delayed quorums");
    second.mark_federation_healthy().await;
    let client =
        FederationRestPeerClient::new(MockFederationTransport::default(), &public, second_private)
            .expect("local promotion client");
    for expected in &checkpoints {
        run_federation_peer_sync_round(&second, &client, &[]).await;
        assert_eq!(
            second_db
                .get_trusted_federation_checkpoint_quorum()
                .await
                .expect("promoted local quorum"),
            Some(expected.clone()),
            "idle peer sync must promote the next stored quorum in sequence"
        );
    }
    assert_eq!(
        first_db
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("first trust"),
        second_db
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("second trust"),
    );
    assert!(first_db
        .get_next_federation_scheduled_checkpoint(3)
        .await
        .expect("bounded end")
        .is_none());
}
