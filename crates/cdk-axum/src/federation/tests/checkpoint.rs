#[test]
fn checkpoint_transfer_budget_rejects_oversize_and_overflow() {
    assert_eq!(
        super::sync_loop::checked_checkpoint_transfer_bytes(40, 2, 42)
            .expect("transfer exactly at budget"),
        42
    );
    assert!(matches!(
        super::sync_loop::checked_checkpoint_transfer_bytes(40, 3, 42),
        Err(FederationError::CheckpointCatchUpTooLarge {
            resource: "transfer batch",
            actual: 43,
            max: 42,
        })
    ));
    assert!(matches!(
        super::sync_loop::checked_checkpoint_transfer_bytes(u64::MAX, 1, u64::MAX),
        Err(FederationError::CheckpointCatchUpTooLarge {
            resource: "transfer batch",
            actual: u64::MAX,
            max: u64::MAX,
        })
    ));
}

#[test]
fn checkpoint_page_byte_limit_trims_only_at_complete_verified_units() {
    let (public_config, local_private, _) = transport_config();
    let mut runtime = FederationConsensusRuntime::authenticated_aleph_bft(
        &public_config,
        local_private.member_id,
        0,
    )
    .expect("large checkpoint page runtime");
    for unit_index in 0..12_u64 {
        let mut envelope = payment_observation_envelope(public_config.federation_id);
        let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
            panic!("mint quote payment observation");
        };
        observation.quote = format!("large-checkpoint-page-{unit_index}");
        observation.payment_proof = Some("x".repeat(700 * 1024));
        envelope = envelope
            .authorize(&local_private)
            .expect("authorize large checkpoint page operation");
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            local_private.member_id,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("large checkpoint page unit");
        runtime
            .finalize_unit(unit)
            .expect("finalize large checkpoint page unit");
    }
    let request = FederationJournalCatchUpRequest::new(0)
        .with_limit(100)
        .with_aleph_bft_next_item_index(0);
    let response = FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        runtime.journal(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
        &request,
        100,
        100,
    )
    .with_aleph_bft_finalized_items(runtime.finalized_items().to_vec(), true);
    assert!(
        response.encoded_size().expect("untrimmed page size")
            > cdk::federation::FEDERATION_MAX_CHECKPOINT_CATCH_UP_PAGE_BYTES
    );

    let fitted = super::catch_up_routes::fit_checkpoint_page_to_byte_limit(response)
        .expect("fit checkpoint page at complete unit");
    fitted
        .ensure_within_checkpoint_page_limit()
        .expect("fitted checkpoint page size");
    assert!(!fitted.entries.is_empty());
    assert!(fitted.entries.len() < 12);
    assert_eq!(
        fitted.entries.len(),
        fitted
            .aleph_bft_finalized_items
            .iter()
            .filter(|item| {
                item.status == FederationAlephBftFinalizedItemStatus::Accepted
                    && item.operation_id.is_some()
            })
            .count()
    );
    assert!(!fitted.complete);
    assert!(!fitted.aleph_bft_finalized_items_complete);
}

#[test]
fn checkpoint_page_cursor_checks_reject_no_progress_and_overflow() {
    assert!(matches!(
        super::sync_loop::checked_checkpoint_page_cursors(7, 9, 0, 0),
        Err(FederationError::CatchUpNoProgress { index: 7 })
    ));
    assert!(matches!(
        super::sync_loop::checked_checkpoint_page_cursors(u64::MAX, 9, 1, 0),
        Err(FederationError::CheckpointInvalid { .. })
    ));
    assert!(matches!(
        super::sync_loop::checked_checkpoint_page_cursors(7, u64::MAX, 0, 1),
        Err(FederationError::CheckpointInvalid { .. })
    ));
}

#[tokio::test]
async fn rest_peer_client_rejects_oversized_backup_before_validation() {
    let (public_config, local_private, peer_private) = transport_config();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("oversized response peer");
    let request = FederationAlephBftBackupRequest::new(0, 0);
    let response = FederationAlephBftBackupResponse::new(
        public_config.federation_id,
        peer_member.member_id,
        request,
        vec![vec![255; 3 * 1024 * 1024]],
        true,
    );
    let transport =
        MockFederationTransport::default().with_aleph_bft_backup_response(response, peer_private);
    let client = FederationRestPeerClient::new_with_config(
        transport,
        &public_config,
        local_private,
        FederationPeerSyncConfig::default(),
    )
    .expect("oversized response client");
    let error = client
        .aleph_bft_backup(peer_member, request)
        .await
        .expect_err("oversized backup response must fail before validation");
    assert!(matches!(
        error.downcast_ref::<FederationError>(),
        Some(FederationError::CheckpointCatchUpTooLarge {
            resource: "decoded response",
            ..
        })
    ));
}

#[tokio::test]
async fn consensus_prepared_delta_is_bounded_by_new_work() {
    let (state, public_config, local_private, _, _) =
        federation_route_state_three_with_local_private().await;
    let mut prior_bytes = 0_usize;
    let mut prepared_bytes = Vec::new();
    for operation_count in [1_usize, 64, 256] {
        let mut runtime = FederationConsensusRuntime::for_members(
            public_config.federation_id,
            local_private.member_id,
            0,
            public_config.members.keys().copied(),
        )
        .expect("prepared delta runtime");
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
            .expect("clone baseline unit");
            runtime
                .finalize_unit(unit)
                .expect("finalize prepared delta baseline unit");
        }
        let retained_bytes =
            serde_json::to_vec(&(runtime.journal().entries(), runtime.finalized_items()))
                .expect("encode retained payload")
                .len();
        let before = runtime.status();
        let envelope = mint_quote_envelope(&state, &QuoteId::new());
        let submission_started = std::time::Instant::now();
        let submission = runtime
            .prepare_aleph_bft_envelope(envelope.clone())
            .expect("prepare submission delta");
        let submission_elapsed = submission_started.elapsed();
        assert_eq!(
            submission.admission(),
            FederationConsensusItemAdmission::Accepted
        );
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            local_private.member_id,
            0,
            runtime.next_unit_index(),
            vec![FederationConsensusItem::operation(envelope.clone())],
        )
        .expect("prepared finalization unit");
        let clone_started = std::time::Instant::now();
        let mut cloned_submission_runtime = runtime.clone();
        cloned_submission_runtime
            .submit_aleph_bft_envelope(envelope.clone())
            .expect("clone-path submission");
        let mut cloned_finalization_runtime = runtime.clone();
        cloned_finalization_runtime
            .finalize_aleph_bft_unit(unit.clone())
            .expect("clone-path finalization");
        let clone_elapsed = clone_started.elapsed();
        let finalization_started = std::time::Instant::now();
        let finalization = std::hint::black_box(
            runtime
                .prepare_aleph_bft_finalization(unit)
                .expect("prepare finalized delta"),
        );
        let elapsed = submission_elapsed.saturating_add(finalization_started.elapsed());
        assert_eq!(runtime.status(), before);
        assert_eq!(finalization.finalized_items().len(), 1);
        assert_eq!(finalization.appended_entries().len(), 1);
        assert_eq!(finalization.responses().len(), 1);
        let delta_bytes = serde_json::to_vec(&(
            finalization.finalized_items(),
            finalization.appended_entries(),
            finalization.responses(),
        ))
        .expect("encode prepared delta")
        .len();
        prepared_bytes.push(delta_bytes);
        assert!(delta_bytes < retained_bytes.saturating_mul(2));
        assert!(retained_bytes > prior_bytes);
        prior_bytes = retained_bytes;
        eprintln!(
            "consensus_prepared_delta operations={operation_count} retained_canonical_bytes={retained_bytes} delta_canonical_bytes={delta_bytes} prepared_elapsed_ns={} clone_path_elapsed_ns={}",
            elapsed.as_nanos(),
            clone_elapsed.as_nanos(),
        );
    }
    let min_delta = prepared_bytes.iter().copied().min().expect("minimum delta");
    let max_delta = prepared_bytes.iter().copied().max().expect("maximum delta");
    assert!(max_delta.saturating_sub(min_delta) < 32);
}

#[tokio::test]
async fn rest_peer_client_pages_catch_up_across_default_limit_boundary() {
    for operation_count in [99_usize, 100, 101] {
        let catch_up_limit = 100;
        let (state, public_config, local_private, peer_private, witness_private) =
            federation_route_state_three_with_local_private().await;
        let peer_member = public_config
            .member(peer_private.member_id)
            .expect("peer member");
        let peer_state =
            federation_route_state_for_private(public_config.clone(), peer_private.clone()).await;
        for unit_index in 0..operation_count as u64 {
            let unit = FederationAlephBftUnitPayload::new(
                public_config.federation_id,
                peer_member.member_id,
                0,
                unit_index,
                vec![FederationConsensusItem::operation(mint_quote_envelope(
                    &peer_state,
                    &QuoteId::new(),
                ))],
            )
            .expect("peer finalized unit");
            peer_state
                .apply_federation_aleph_bft_finalized_unit(unit)
                .await
                .expect("apply peer unit");
        }
        let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
            &peer_state,
            &public_config,
            None,
            &[&peer_private, &witness_private],
        )
        .await;
        let checkpoint_id = checkpoint.id().expect("genesis catch-up checkpoint id");
        let peer_consensus = peer_state
            .federation
            .consensus
            .as_ref()
            .expect("peer consensus")
            .lock()
            .await
            .clone();
        let mut transport = MockFederationTransport::default();
        let mut from = 0_usize;
        let mut page_count = 0_usize;
        while from < operation_count {
            let request = FederationJournalCatchUpRequest::new(from as u64)
                .with_target_checkpoint(checkpoint_id)
                .with_limit(catch_up_limit)
                .with_aleph_bft_next_item_index(from as u64);
            let response = FederationJournalCatchUpResponse::from_journal(
                public_config.federation_id,
                peer_consensus.journal(),
                checkpoint.materialized_state_digest,
                &request,
                100,
                1000,
            )
            .bound_to_checkpoint(&checkpoint)
            .expect("bound genesis catch-up page");
            let end = from.saturating_add(response.entries.len());
            let response = response.with_aleph_bft_finalized_items(
                peer_consensus.finalized_items()[from..end].to_vec(),
                end == operation_count,
            );
            transport = transport.with_catch_up_response(response);
            from = end;
            page_count = page_count.saturating_add(1);
        }

        let client = FederationRestPeerClient::new_with_config(
            transport.clone(),
            &public_config,
            local_private,
            FederationPeerSyncConfig {
                catch_up_limit,
                ..FederationPeerSyncConfig::default()
            },
        )
        .expect("peer client");

        time::timeout(
            Duration::from_secs(20),
            client.catch_up_state_to_checkpoint_from_member(&state, peer_member, &quorum),
        )
        .await
        .expect("paged catch-up completed within bound")
        .expect("paged catch-up succeeded");

        let local_status = local_peer_status(&state).await.expect("local status");
        assert_eq!(local_status.operation_count, operation_count as u64);
        assert_eq!(
            local_status.latest_order_digest,
            checkpoint.journal_order_digest
        );
        assert_eq!(
            local_status.aleph_bft_next_item_index,
            Some(operation_count as u64)
        );
        let trusted = state
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted genesis catch-up checkpoint")
            .expect("promoted genesis catch-up checkpoint");
        assert_eq!(
            trusted.checkpoint.id().expect("trusted checkpoint id"),
            checkpoint_id
        );
        assert_eq!(transport.catch_up_requests().len(), page_count);
        eprintln!(
            "genesis_catch_up_baseline operations={operation_count} page_limit={catch_up_limit} pages={page_count} requests={}",
            transport.catch_up_requests().len()
        );
    }
}

#[tokio::test]
async fn checkpoint_catch_up_resumes_after_each_durable_page_restart() {
    let catch_up_limit = 2;
    let operation_count = 5_usize;
    let (public_config, local_private, peer_private, witness_private) = transport_config_three();
    let peer_member = public_config
        .member(peer_private.member_id)
        .expect("peer member");
    let peer_state =
        federation_route_state_for_private(public_config.clone(), peer_private.clone()).await;
    for unit_index in 0..operation_count as u64 {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            peer_member.member_id,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(mint_quote_envelope(
                &peer_state,
                &QuoteId::new(),
            ))],
        )
        .expect("restart catch-up unit");
        peer_state
            .apply_federation_aleph_bft_finalized_unit(unit)
            .await
            .expect("apply restart catch-up unit");
    }
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &peer_state,
        &public_config,
        None,
        &[&peer_private, &witness_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("restart checkpoint id");
    let peer_consensus = peer_state
        .federation
        .consensus
        .as_ref()
        .expect("peer consensus")
        .lock()
        .await
        .clone();
    let pages = one_operation_unit_checkpoint_pages(&peer_consensus, &checkpoint, catch_up_limit);
    assert_eq!(pages.len(), 3);

    for crash_after_pages in 1..pages.len() {
        let mut state =
            federation_route_state_for_private(public_config.clone(), local_private.clone()).await;
        for page in pages.iter().take(crash_after_pages) {
            state
                .apply_federation_checkpoint_catch_up_response(page, &quorum)
                .await
                .expect("apply durable catch-up page before restart");
        }
        let durable_cursor =
            u64::try_from(crash_after_pages.saturating_mul(catch_up_limit as usize))
                .expect("durable page cursor fits u64");
        let frontier = state
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("durable catch-up frontier");
        assert_eq!(frontier.journal_next_index, durable_cursor);
        assert_eq!(frontier.application_next_index, durable_cursor);
        assert!(state
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted checkpoint before restart")
            .is_none());

        let (restarted_consensus, restarted_sequences) =
            crate::create_federation_runtime_state(&state.mint)
                .await
                .expect("restart from durable checkpoint page");
        state.federation.consensus = restarted_consensus;
        state.federation.sequences = restarted_sequences;

        let mut transport = MockFederationTransport::default();
        for page in pages.iter().skip(crash_after_pages).cloned() {
            transport = transport.with_catch_up_response(page);
        }
        let client = FederationRestPeerClient::new_with_config(
            transport.clone(),
            &public_config,
            local_private.clone(),
            FederationPeerSyncConfig {
                catch_up_limit,
                ..FederationPeerSyncConfig::default()
            },
        )
        .expect("restart peer client");
        client
            .catch_up_state_to_checkpoint_from_member(&state, peer_member, &quorum)
            .await
            .expect("resume checkpoint catch-up after restart");

        let requests = transport.catch_up_requests();
        assert_eq!(
            requests.len(),
            pages.len().saturating_sub(crash_after_pages)
        );
        assert_eq!(requests[0].payload.next_index, durable_cursor);
        assert_eq!(
            requests[0].payload.aleph_bft_next_item_index,
            durable_cursor
        );
        let trusted = state
            .mint
            .localstore()
            .get_trusted_federation_checkpoint_quorum()
            .await
            .expect("trusted resumed checkpoint")
            .expect("promoted resumed checkpoint");
        assert_eq!(
            trusted.checkpoint.id().expect("trusted checkpoint id"),
            checkpoint_id
        );
    }
}

#[tokio::test]
async fn one_page_and_many_page_checkpoint_catch_up_produce_identical_state() {
    let operation_count = 6_usize;
    let (public_config, local_private, source_private, witness_private) = transport_config_three();
    let seed = [73_u8; 64];
    let source = federation_route_state_for_private_with_db(
        public_config.clone(),
        source_private.clone(),
        Arc::new(cdk_sqlite::mint::memory::empty().await.expect("source db")),
        &seed,
    )
    .await;
    let mut quote_ids = Vec::new();
    for _ in 0..operation_count {
        let quote_id = QuoteId::new();
        source
            .submit_federation_envelope(mint_quote_envelope(&source, &quote_id))
            .await
            .expect("source page-equivalence operation");
        quote_ids.push(quote_id);
    }
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &source,
        &public_config,
        None,
        &[&source_private, &witness_private],
    )
    .await;
    let source_runtime = source
        .federation
        .consensus
        .as_ref()
        .expect("source page-equivalence consensus")
        .lock()
        .await
        .clone();
    let one_page =
        one_operation_unit_checkpoint_pages(&source_runtime, &checkpoint, operation_count as u16);
    let many_pages = one_operation_unit_checkpoint_pages(&source_runtime, &checkpoint, 2);
    assert_eq!(one_page.len(), 1);
    assert_eq!(many_pages.len(), 3);

    let one_page_target = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("one-page target db"),
        ),
        &seed,
    )
    .await;
    let many_page_target = federation_route_state_for_private_with_db(
        public_config.clone(),
        local_private.clone(),
        Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("many-page target db"),
        ),
        &seed,
    )
    .await;
    let source_member = public_config
        .member(source_private.member_id)
        .expect("source member");
    let one_transport = MockFederationTransport::default()
        .with_catch_up_response(one_page.into_iter().next().expect("one checkpoint page"));
    let mut many_transport = MockFederationTransport::default();
    for page in many_pages {
        many_transport = many_transport.with_catch_up_response(page);
    }
    let one_client = FederationRestPeerClient::new_with_config(
        one_transport,
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig {
            catch_up_limit: operation_count as u16,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("one-page client");
    let many_client = FederationRestPeerClient::new_with_config(
        many_transport,
        &public_config,
        local_private,
        FederationPeerSyncConfig {
            catch_up_limit: 2,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("many-page client");
    one_client
        .catch_up_state_to_checkpoint_from_member(&one_page_target, source_member, &quorum)
        .await
        .expect("one-page checkpoint catch-up");
    many_client
        .catch_up_state_to_checkpoint_from_member(&many_page_target, source_member, &quorum)
        .await
        .expect("many-page checkpoint catch-up");

    assert_eq!(
        materialized_state_snapshot(&one_page_target)
            .await
            .expect("one-page materialized state"),
        materialized_state_snapshot(&many_page_target)
            .await
            .expect("many-page materialized state")
    );
    assert_eq!(
        one_page_target
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("one-page frontier"),
        many_page_target
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("many-page frontier")
    );
    for quote_id in quote_ids {
        assert!(one_page_target
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("one-page quote")
            .is_some());
        assert!(many_page_target
            .mint
            .localstore()
            .get_mint_quote(&quote_id)
            .await
            .expect("many-page quote")
            .is_some());
    }
}

#[tokio::test]
async fn checkpoint_catch_up_rejects_bad_source_then_uses_second_source() {
    let operation_count = 3_usize;
    let (state, public_config, local_private, source_private, second_source_private) =
        federation_route_state_three_with_local_private().await;
    let source_member = public_config
        .member(source_private.member_id)
        .expect("source member");
    let second_source_member = public_config
        .member(second_source_private.member_id)
        .expect("second source member");
    let source_state =
        federation_route_state_for_private(public_config.clone(), source_private.clone()).await;
    for unit_index in 0..operation_count as u64 {
        let unit = FederationAlephBftUnitPayload::new(
            public_config.federation_id,
            source_member.member_id,
            0,
            unit_index,
            vec![FederationConsensusItem::operation(mint_quote_envelope(
                &source_state,
                &QuoteId::new(),
            ))],
        )
        .expect("source failover unit");
        source_state
            .apply_federation_aleph_bft_finalized_unit(unit)
            .await
            .expect("apply source failover unit");
    }
    let (checkpoint, quorum) = recovery_checkpoint_quorum_for_state(
        &source_state,
        &public_config,
        None,
        &[&source_private, &second_source_private],
    )
    .await;
    let checkpoint_id = checkpoint.id().expect("source failover checkpoint id");
    let source_consensus = source_state
        .federation
        .consensus
        .as_ref()
        .expect("source consensus")
        .lock()
        .await
        .clone();
    let request = FederationJournalCatchUpRequest::new(0)
        .with_target_checkpoint(checkpoint_id)
        .with_aleph_bft_next_item_index(0);
    let response = FederationJournalCatchUpResponse::from_journal(
        public_config.federation_id,
        source_consensus.journal(),
        checkpoint.materialized_state_digest,
        &request,
        100,
        100,
    )
    .bound_to_checkpoint(&checkpoint)
    .expect("bound source failover response")
    .with_aleph_bft_finalized_items(source_consensus.finalized_items().to_vec(), true);
    let mut invalid_response = response.clone();
    invalid_response.entries[0].operation_id = FederationOperationId::from_bytes([91; 32]);

    let bad_transport = MockFederationTransport::default().with_catch_up_response(invalid_response);
    let bad_client = FederationRestPeerClient::new_with_config(
        bad_transport.clone(),
        &public_config,
        local_private.clone(),
        FederationPeerSyncConfig::default(),
    )
    .expect("bad source client");
    bad_client
        .catch_up_state_to_checkpoint_from_member(&state, source_member, &quorum)
        .await
        .expect_err("invalid first source must fail closed");
    let rejected_frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("frontier after invalid source");
    assert_eq!(rejected_frontier.journal_next_index, 0);
    assert_eq!(rejected_frontier.finalized_next_item_index, 0);
    assert!(state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint after invalid source")
        .is_none());

    let good_transport = MockFederationTransport::default().with_catch_up_response(response);
    let good_client = FederationRestPeerClient::new_with_config(
        good_transport.clone(),
        &public_config,
        local_private,
        FederationPeerSyncConfig::default(),
    )
    .expect("second source client");
    good_client
        .catch_up_state_to_checkpoint_from_member(&state, second_source_member, &quorum)
        .await
        .expect("second source completes checkpoint catch-up");
    assert_eq!(bad_transport.catch_up_requests().len(), 1);
    assert_eq!(good_transport.catch_up_requests().len(), 1);
    let trusted = state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint after source failover")
        .expect("checkpoint promoted after source failover");
    assert_eq!(
        trusted.checkpoint.id().expect("trusted checkpoint id"),
        checkpoint_id
    );
}

