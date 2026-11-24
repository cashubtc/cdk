//! Startup quorum reconciliation and replay targets.

use super::*;

#[test]
fn quorum_startup_requires_threshold_matching_views() {
    let (public_config, _, _) = transport_config();
    let local = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        member_id(1),
        0,
        None,
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    let matching_peer = |member| {
        let mut status = local.clone();
        status.member_id = member_id(member);
        status
    };
    let peers = vec![matching_peer(2)];

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &peers, 2),
        Some(0)
    );
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &peers, 3),
        None
    );
}

#[test]
fn quorum_startup_adopts_only_a_matching_later_session() {
    let (public_config, _, _) = transport_config();
    let local = FederationPeerStatus::new_with_materialized_state_digest(
        public_config.federation_id,
        member_id(1),
        7,
        Some(6),
        FederationJournalDigest::genesis(),
        cdk::federation::FederationMaterializedStateDigest::genesis(),
    );
    let later_peer = |member| {
        let mut status = local.clone();
        status.member_id = member_id(member);
        status.aleph_bft_session = 2;
        status
    };
    let mut divergent = later_peer(3);
    divergent.operation_count = 8;

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[later_peer(2)], 2),
        Some(2)
    );
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[divergent], 2),
        None
    );
}

#[test]
fn quorum_startup_ignores_non_application_finality_and_local_candidate_audits() {
    let (public_config, _, _) = transport_config();
    let mut local = FederationPeerStatus::new(
        public_config.federation_id,
        member_id(1),
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_next_item_index(40);
    local.application_digest = Some(cdk::federation::FederationApplicationDigest::genesis());
    local.latest_audited_checkpoint_id =
        Some(cdk::federation::FederationCheckpointId::from_bytes([1; 32]));
    local.materialized_state_digest =
        Some(cdk::federation::FederationMaterializedStateDigest::from_bytes([2; 32]));

    let mut peer = local.clone();
    peer.member_id = member_id(2);
    peer.aleph_bft_next_item_index = Some(44);
    peer.latest_audited_checkpoint_id =
        Some(cdk::federation::FederationCheckpointId::from_bytes([3; 32]));
    peer.materialized_state_digest =
        Some(cdk::federation::FederationMaterializedStateDigest::from_bytes([4; 32]));

    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[peer.clone()], 2),
        Some(0)
    );

    peer.application_digest = Some(cdk::federation::FederationApplicationDigest::from_bytes(
        [5; 32],
    ));
    assert_eq!(
        sync_loop::quorum_compatible_session(&local, &[peer], 2),
        None
    );
}

#[test]
fn quorum_replay_target_uses_supported_current_session_frontier() {
    let (public_config, _) = transport_config_five_with_threshold(
        ThresholdParams::new(5, 3, 4).expect("threshold params"),
    );
    let members = public_config.members.keys().copied().collect::<Vec<_>>();
    let local_member = members[0];
    let tracked_member = members[1];
    let local_cursor = members.iter().copied().map(|member| (member, 5)).collect();
    let local = FederationPeerStatus::new(
        public_config.federation_id,
        local_member,
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_session(3)
    .with_aleph_bft_next_unit_index_by_member(local_cursor);
    let peer = |member: MemberId, tracked_next_unit_index: u64| {
        let mut status = local.clone();
        status.member_id = member;
        status
            .aleph_bft_next_unit_index_by_member
            .insert(tracked_member, tracked_next_unit_index);
        status
    };
    let mut stale_session = peer(members[4], 100);
    stale_session.aleph_bft_session = 2;
    let peer_statuses = vec![
        peer(members[1], 11),
        peer(members[2], 10),
        peer(members[3], 9),
        peer(members[4], 1),
        stale_session,
    ];

    let target = sync_loop::quorum_supported_replay_target(
        &local,
        &peer_statuses,
        3,
        usize::from(public_config.threshold_params.consensus_threshold),
    );

    assert_eq!(target.get(&tracked_member), Some(&5));
    assert_eq!(target.get(&local_member), Some(&5));
}

#[test]
fn quorum_replay_target_ignores_a_single_peers_unsupported_high_cursor() {
    let (public_config, first_private, second_private, third_private) =
        transport_config_three_with_threshold(
            ThresholdParams::new(3, 2, 2).expect("threshold params"),
        );
    let members = [&first_private, &second_private, &third_private]
        .into_iter()
        .map(|private| private.member_id)
        .collect::<Vec<_>>();
    let tracked_member = members[1];
    let mut local = FederationPeerStatus::new(
        public_config.federation_id,
        members[0],
        7,
        Some(6),
        FederationJournalDigest::genesis(),
    )
    .with_aleph_bft_session(3)
    .with_aleph_bft_next_unit_index_by_member(
        members.iter().copied().map(|member| (member, 5)).collect(),
    );
    local.application_digest = Some(cdk::federation::FederationApplicationDigest::genesis());
    let mut unsupported = local.clone();
    unsupported.member_id = members[1];
    unsupported
        .aleph_bft_next_unit_index_by_member
        .insert(tracked_member, 1_000_000);

    let target = sync_loop::quorum_supported_replay_target(&local, &[unsupported], 3, 2);

    assert_eq!(target.get(&tracked_member), Some(&5));
}

#[test]
fn quorum_startup_rejects_stale_round_evidence() {
    let observed_at = tokio::time::Instant::now();
    let max_age = std::time::Duration::from_secs(10);

    assert!(sync_loop::quorum_evidence_is_current(
        observed_at,
        observed_at + max_age,
        max_age,
    ));
    assert!(!sync_loop::quorum_evidence_is_current(
        observed_at,
        observed_at + max_age + std::time::Duration::from_millis(1),
        max_age,
    ));
}

#[test]
fn recovery_target_ignores_proof_superseded_by_scheduled_checkpoint() {
    assert!(sync_loop::recovery_checkpoint_is_superseded(1, 2));
    assert!(!sync_loop::recovery_checkpoint_is_superseded(2, 2));
    assert!(!sync_loop::recovery_checkpoint_is_superseded(3, 2));
}

#[tokio::test]
async fn checkpoint_target_ignores_newer_frontier_on_superseded_recovery_lineage() {
    let (state, public_config, peer_private) = federation_route_state().await;
    let local_private = state
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let signers = [&local_private, &peer_private];

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("first scheduled operation");
    let mut first_scheduled = recovery_checkpoint_for_state(&state, &public_config, None).await;
    first_scheduled.kind = FederationCheckpointKind::Scheduled;
    first_scheduled.scheduled_checkpoint_sequence = 1;
    let first_quorum = FederationCheckpointQuorum::new(
        first_scheduled.clone(),
        signers
            .iter()
            .map(|signer| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&first_scheduled, signer)
                    .expect("first scheduled signature")
            })
            .collect(),
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("second scheduled operation");
    let mut second_scheduled =
        recovery_checkpoint_for_state(&state, &public_config, Some(&first_scheduled)).await;
    second_scheduled.kind = FederationCheckpointKind::Scheduled;
    second_scheduled.scheduled_checkpoint_sequence = 2;
    let second_quorum = FederationCheckpointQuorum::new(
        second_scheduled.clone(),
        signers
            .iter()
            .map(|signer| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&second_scheduled, signer)
                    .expect("second scheduled signature")
            })
            .collect(),
    );

    state
        .submit_federation_envelope(mint_quote_envelope(&state, &QuoteId::new()))
        .await
        .expect("stale-lineage recovery operation");
    let (stale_recovery, stale_recovery_quorum) = recovery_checkpoint_quorum_for_state(
        &state,
        &public_config,
        Some(&first_scheduled),
        &signers,
    )
    .await;
    assert!(stale_recovery.journal_next_index > second_scheduled.journal_next_index);
    assert!(
        !checkpoint_manager::checkpoint_covers_current_recovery_need(
            &stale_recovery,
            stale_recovery.finalized_next_item_index,
            None,
            second_scheduled.scheduled_checkpoint_sequence,
            Some(
                second_scheduled
                    .id()
                    .expect("second scheduled checkpoint id")
            ),
        )
    );

    let db = state.mint.localstore();
    let mut tx = db
        .begin_transaction()
        .await
        .expect("begin checkpoint setup");
    tx.promote_verified_federation_checkpoint(
        &first_quorum
            .verify(&public_config)
            .expect("first scheduled quorum"),
    )
    .await
    .expect("promote first scheduled checkpoint");
    tx.promote_verified_federation_checkpoint(
        &second_quorum
            .verify(&public_config)
            .expect("second scheduled quorum"),
    )
    .await
    .expect("promote second scheduled checkpoint");
    tx.add_federation_checkpoint_quorum(&stale_recovery_quorum)
        .await
        .expect("store stale recovery quorum");
    tx.commit().await.expect("commit checkpoint setup");

    assert!(sync_loop::next_local_checkpoint_catch_up_target(&state)
        .await
        .expect("inspect next checkpoint target")
        .is_none());
}

#[test]
fn live_healthy_member_does_not_reenter_startup_quorum_reconciliation() {
    assert!(!sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Healthy,
        true,
    ));
    assert!(sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Healthy,
        false,
    ));
    assert!(sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::AwaitingQuorum { operation_count: 7 },
        true,
    ));
    assert!(!sync_loop::quorum_reconciliation_is_required(
        &crate::FederationSyncStatus::Halted {
            reason: "test halt".to_string(),
        },
        false,
    ));
}
