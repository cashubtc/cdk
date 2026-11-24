//! Recovery checkpoint demand, ordered barriers and their quorums.

use super::*;

#[tokio::test]
async fn recovery_checkpoint_demand_uses_a_bounded_local_proposal_lease() {
    let runtime = FederationNodeRuntime::disabled();
    assert!(!runtime.recovery_checkpoint_is_requested().await);
    runtime.request_recovery_checkpoint().await;
    assert!(runtime.recovery_checkpoint_is_requested().await);
    let now = std::time::Instant::now();
    assert!(
        runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now, Duration::from_secs(30))
            .await
    );
    assert!(
        !runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now + Duration::from_secs(29), Duration::from_secs(30))
            .await
    );
    assert!(
        runtime
            .components
            .checkpoint
            .reserve_recovery_proposal(now + Duration::from_secs(30), Duration::from_secs(30))
            .await
    );
}

#[tokio::test]
async fn idle_member_does_not_propose_recovery_barrier_work() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config, first_private).await;

    assert!(!state.federation.recovery_checkpoint_is_requested().await);
    assert!(!state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("idle recovery proposal"));
    assert!(!state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .has_pending_aleph_bft_recovery_barrier());
}

#[tokio::test]
async fn independent_members_derive_identical_recovery_checkpoint_at_terminal_boundary() {
    let (public_config, first_private, second_private) = transport_config();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let items = (0..2)
        .map(|_| FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())))
        .collect();
    time::timeout(
        Duration::from_secs(15),
        apply_identical_finalized_unit(&first, &second, items),
    )
    .await
    .expect("stable-tip unit application timeout");

    let first_checkpoint = time::timeout(
        Duration::from_secs(15),
        recovery_checkpoint_for_state(&first, &public_config, None),
    )
    .await
    .expect("first recovery-checkpoint timeout");
    let second_checkpoint = time::timeout(
        Duration::from_secs(15),
        recovery_checkpoint_for_state(&second, &public_config, None),
    )
    .await
    .expect("second recovery-checkpoint timeout");
    assert_eq!(first_checkpoint, second_checkpoint);
    let quorum = FederationCheckpointQuorum::new(
        first_checkpoint.clone(),
        vec![
            FederationCheckpointSignature::new_signed(&first_checkpoint, &first_private)
                .expect("first recovery-checkpoint signature"),
            FederationCheckpointSignature::new_signed(&second_checkpoint, &second_private)
                .expect("second recovery-checkpoint signature"),
        ],
    );
    quorum
        .verify(&public_config)
        .expect("stable-tip recovery-checkpoint quorum");
    assert_eq!(first_checkpoint.journal_next_index, 2);
    assert_eq!(first_checkpoint.finalized_next_item_index, 2);
    assert_eq!(first_checkpoint.next_unit_finalization_index, 1);
    assert_eq!(
        first_checkpoint.materialized_state_digest,
        first
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("first stable-tip materialized state")
            .digest()
            .expect("first stable-tip materialized digest")
    );
    assert_eq!(
        first_checkpoint.materialized_state_digest,
        second
            .mint
            .federation_materialized_state_snapshot()
            .await
            .expect("second stable-tip materialized state")
            .digest()
            .expect("second stable-tip materialized digest")
    );
}

#[tokio::test]
async fn ordered_recovery_barrier_captures_complete_unit_without_stopping_traffic() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), first_private).await;
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    let payload = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![
                FederationConsensusItem::operation(mint_quote_envelope(&state, &first_quote)),
                FederationConsensusItem::recovery_barrier(
                    FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
                        .expect("recovery barrier"),
                ),
                FederationConsensusItem::operation(mint_quote_envelope(&state, &second_quote)),
            ],
        )
        .expect("barrier unit")
    };

    let responses = state
        .federation
        .apply_finalized_unit(payload)
        .await
        .expect("finalize barrier unit");
    assert_eq!(responses.len(), 2);
    assert!(responses.iter().all(|response| matches!(
        response.outcome.entry().status(),
        FederationOperationStatus::Applied | FederationOperationStatus::Rejected
    )));
    let checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .expect("recovery checkpoint candidate");
    assert_eq!(checkpoint.journal_next_index, 2);
    assert_eq!(checkpoint.finalized_next_item_index, 3);
    assert_eq!(checkpoint.next_unit_finalization_index, 1);
    let checkpoint_id = checkpoint.id().expect("initial recovery checkpoint id");
    assert!(state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint lookup")
        .is_none());

    let finality_only_unit = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::ModuleFact(
                FederationConsensusModuleFact::new(
                    "checkpoint-test",
                    "finality-only",
                    serde_json::json!({}),
                ),
            )],
        )
        .expect("finality-only unit")
    };
    state
        .federation
        .apply_finalized_unit(finality_only_unit)
        .await
        .expect("finalize non-application item");
    let redundant_barrier = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::recovery_barrier(
                FederationRecoveryBarrier::new(
                    public_config.federation_id,
                    0,
                    None,
                    status.next_item_index,
                )
                .expect("redundant recovery barrier"),
            )],
        )
        .expect("redundant barrier unit")
    };
    state
        .federation
        .apply_finalized_unit(redundant_barrier)
        .await
        .expect("finalize redundant barrier");
    let retained_checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
        .await
        .expect("retained recovery checkpoint lookup")
        .expect("retained recovery checkpoint");
    assert_eq!(
        retained_checkpoint
            .id()
            .expect("retained recovery checkpoint id"),
        checkpoint_id
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("operation after recovery barrier");
    assert_eq!(consensus_operation_count(&state).await, 3);

    let repeated_barrier = {
        let consensus = state
            .federation
            .consensus()
            .expect("consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![FederationConsensusItem::recovery_barrier(
                FederationRecoveryBarrier::new(
                    public_config.federation_id,
                    0,
                    None,
                    status.next_item_index,
                )
                .expect("repeated recovery barrier"),
            )],
        )
        .expect("repeated barrier unit")
    };
    state
        .federation
        .apply_finalized_unit(repeated_barrier)
        .await
        .expect("finalize repeated barrier");
    let advanced_checkpoint = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("advanced recovery checkpoint lookup")
        .expect("advanced recovery checkpoint");
    assert_ne!(
        advanced_checkpoint
            .id()
            .expect("advanced recovery checkpoint id"),
        checkpoint_id
    );
    assert_eq!(advanced_checkpoint.journal_next_index, 3);
    assert_eq!(advanced_checkpoint.finalized_next_item_index, 7);
}

#[tokio::test]
async fn recovery_barrier_reaches_quorum_with_one_member_unavailable() {
    let (public_config, first_private, second_private, _unavailable_private) =
        transport_config_three();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let items = vec![
        FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())),
        FederationConsensusItem::recovery_barrier(
            FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
                .expect("recovery barrier"),
        ),
        FederationConsensusItem::operation(mint_quote_envelope(&first, &QuoteId::new())),
    ];
    apply_identical_finalized_unit(&first, &second, items).await;

    let first_checkpoint = first
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("first checkpoint lookup")
        .expect("first checkpoint");
    let second_checkpoint = second
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("second checkpoint lookup")
        .expect("second checkpoint");
    assert_eq!(
        first_checkpoint.id().expect("first checkpoint id"),
        second_checkpoint.id().expect("second checkpoint id")
    );
    FederationCheckpointQuorum::new(
        first_checkpoint.clone(),
        vec![
            FederationCheckpointSignature::new_signed(&first_checkpoint, &first_private)
                .expect("first signature"),
            FederationCheckpointSignature::new_signed(&second_checkpoint, &second_private)
                .expect("second signature"),
        ],
    )
    .verify(&public_config)
    .expect("two-of-three recovery checkpoint quorum");
}

#[tokio::test]
async fn concurrent_recovery_requests_queue_one_effective_barrier() {
    let (public_config, first_private, _) = transport_config();
    let state = federation_route_state_for_private(public_config, first_private).await;
    state.federation.request_recovery_checkpoint().await;

    let (first, second) = tokio::join!(
        state.federation.propose_recovery_barrier_if_requested(),
        state.federation.propose_recovery_barrier_if_requested(),
    );
    assert_eq!(
        usize::from(first.expect("first proposal")) + usize::from(second.expect("second proposal")),
        1
    );
    let consensus = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await;
    assert_eq!(
        consensus
            .aleph_bft_finalized_items()
            .expect("finalized items")
            .iter()
            .filter(|item| matches!(item.item, FederationConsensusItem::RecoveryBarrier(_)))
            .count(),
        1
    );
    drop(consensus);
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .is_some());
}

#[tokio::test]
async fn stale_pending_barrier_does_not_block_current_recovery_request() {
    let (public_config, first_private, second_private) = transport_config();
    let state = federation_route_state_for_private(public_config.clone(), first_private).await;
    let stale = FederationRecoveryBarrier::new(public_config.federation_id, 0, None, 0)
        .expect("stale recovery barrier");
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .submit_aleph_bft_recovery_barrier(stale)
        .expect("queue stale recovery barrier");

    let advancing_unit = FederationAlephBftUnitPayload::new(
        public_config.federation_id,
        second_private.member_id,
        0,
        0,
        vec![FederationConsensusItem::operation(mint_quote_envelope(
            &state,
            &QuoteId::new(),
        ))],
    )
    .expect("advancing unit");
    state
        .federation
        .apply_finalized_unit(advancing_unit)
        .await
        .expect("advance finalized frontier");

    state.federation.request_recovery_checkpoint().await;
    assert!(state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("current recovery proposal"));

    let finalized_barriers = state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .aleph_bft_finalized_items()
        .expect("finalized items")
        .iter()
        .filter(|item| matches!(item.item, FederationConsensusItem::RecoveryBarrier(_)))
        .count();
    assert_eq!(finalized_barriers, 2);
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Recovery)
        .await
        .expect("recovery checkpoint lookup")
        .is_some());
}

#[tokio::test]
async fn restart_retries_last_ordered_barrier_with_the_same_checkpoint_id() {
    let (public_config, first_private, _) = transport_config();
    let mut state = federation_route_state_for_private(public_config.clone(), first_private).await;
    state.federation.suppress_next_recovery_checkpoint_capture();
    state.federation.request_recovery_checkpoint().await;
    assert!(state
        .federation
        .propose_recovery_barrier_if_requested()
        .await
        .expect("barrier proposal"));
    assert!(state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("suppressed checkpoint lookup")
        .is_none());
    let expected_id = recovery_checkpoint_for_state(&state, &public_config, None)
        .await
        .id()
        .expect("expected recovery checkpoint id");

    let (restarted_consensus, _) = crate::create_federation_runtime_state(&state.mint)
        .await
        .expect("restart consensus from durable finalized barrier");
    state.federation.member_mut_for_test().consensus =
        restarted_consensus.expect("configured member handle");

    state
        .federation
        .recover_recovery_checkpoint_candidate_at_startup()
        .await
        .expect("startup recovery checkpoint retry");
    let first_id = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("recovered checkpoint lookup")
        .expect("recovered checkpoint")
        .id()
        .expect("recovered checkpoint id");
    assert_eq!(first_id, expected_id);
    state
        .federation
        .recover_recovery_checkpoint_candidate_at_startup()
        .await
        .expect("idempotent startup retry");
    let repeated_id = state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(cdk::federation::FederationCheckpointKind::Recovery)
        .await
        .expect("repeated checkpoint lookup")
        .expect("repeated checkpoint")
        .id()
        .expect("repeated checkpoint id");
    assert_eq!(first_id, repeated_id);
}
