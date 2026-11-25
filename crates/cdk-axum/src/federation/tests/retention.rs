//! Retention of consensus history below the retention floor.
//!
//! Operations are spread over sessions further apart than the validity
//! window, so the validity window of a scheduled checkpoint starts at the
//! previous one and the floor lands exactly on the previous checkpoint.

use cdk::federation::{
    FederationCheckpointFrontier, FederationCheckpointId, FederationOperationId,
    FederationRetentionFloor, FEDERATION_OPERATION_VALIDITY_SESSIONS,
};
use cdk_common::mint::{MeltSagaState, Saga};
use cdk_federation::retention::is_peer_history_pruned;
use cdk_federation::sync_loop::run_federation_peer_sync_round;

use super::*;

/// Operations between scheduled checkpoints.
const CHECKPOINT_INTERVAL: u64 = 2;

/// Sessions between two spaced operations: more than the validity window.
const SESSION_STEP: u64 = FEDERATION_OPERATION_VALIDITY_SESSIONS + 4;

fn retention_policy() -> FederationCheckpointPolicy {
    FederationCheckpointPolicy::new(CHECKPOINT_INTERVAL, CHECKPOINT_INTERVAL)
        .expect("checkpoint policy")
}

/// Sessions of `count` operations spaced `SESSION_STEP` apart, starting with
/// operation `first`.
fn spaced_sessions(first: u64, count: u64) -> Vec<u64> {
    (first..first + count).map(|op| op * SESSION_STEP).collect()
}

async fn live_digest(state: &MintState) -> cdk::federation::FederationMaterializedStateDigest {
    state
        .mint
        .federation_materialized_state_snapshot()
        .await
        .expect("materialized state")
        .digest()
        .expect("materialized digest")
}

async fn current_session(state: &MintState) -> u64 {
    state
        .federation
        .consensus()
        .expect("consensus")
        .lock()
        .await
        .status()
        .session
}

async fn latest_scheduled(state: &MintState) -> Option<FederationCheckpoint> {
    state
        .mint
        .localstore()
        .get_latest_federation_checkpoint(FederationCheckpointKind::Scheduled)
        .await
        .expect("scheduled checkpoint lookup")
}

async fn retention_floor(state: &MintState) -> Option<FederationRetentionFloor> {
    state
        .mint
        .localstore()
        .get_federation_retention_floor()
        .await
        .expect("retention floor lookup")
}

async fn frontier(state: &MintState) -> FederationCheckpointFrontier {
    state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("checkpoint frontier")
}

async fn journal_indexes(state: &MintState) -> Vec<u64> {
    state
        .mint
        .localstore()
        .get_federation_journal_entries()
        .await
        .expect("journal entries")
        .iter()
        .map(|entry| entry.index)
        .collect()
}

async fn item_indexes(state: &MintState) -> Vec<u64> {
    state
        .mint
        .localstore()
        .get_federation_aleph_bft_finalized_items()
        .await
        .expect("finalized items")
        .iter()
        .map(|item| item.accepted_item_index)
        .collect()
}

async fn trusted_checkpoint_id(state: &MintState) -> Option<FederationCheckpointId> {
    state
        .mint
        .localstore()
        .get_trusted_federation_checkpoint_quorum()
        .await
        .expect("trusted checkpoint lookup")
        .map(|quorum| quorum.checkpoint.id().expect("trusted checkpoint id"))
}

/// Order one mint quote in `session` on every member, as one finalized unit.
async fn order_quote(members: &[&MintState], session: u64) -> FederationOperationId {
    let envelope = mint_quote_envelope(members[0], &QuoteId::new());
    order_envelope(members, session, envelope).await
}

/// Order `envelope`, authored by the first member, in `session` on every
/// member, as one finalized unit.
async fn order_envelope(
    members: &[&MintState],
    session: u64,
    envelope: FederationOperationEnvelope,
) -> FederationOperationId {
    if session > current_session(members[0]).await {
        for state in members {
            state
                .federation
                .consensus()
                .expect("consensus")
                .lock()
                .await
                .begin_aleph_bft_session(session)
                .expect("begin session");
        }
    }
    let author = members[0]
        .mint
        .federation_config()
        .expect("federation config")
        .private_config()
        .clone();
    let envelope = envelope
        .for_session(session, &author)
        .expect("stamp operation for its session");
    let operation_id = envelope.operation_id().expect("operation id");
    let payload = {
        let consensus = members[0]
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
            vec![FederationConsensusItem::operation(envelope)],
        )
        .expect("unit payload")
    };
    for state in members {
        state
            .federation
            .apply_finalized_unit(payload.clone())
            .await
            .expect("apply finalized unit");
    }
    operation_id
}

/// Hand every member the other members' signatures on the latest scheduled
/// checkpoint. Each member forms the quorum and promotes it, which prunes.
async fn promote_latest_scheduled(members: &[&MintState]) -> FederationCheckpointQuorum {
    let checkpoint = latest_scheduled(members[0])
        .await
        .expect("scheduled checkpoint");
    let checkpoint_id = checkpoint.id().expect("checkpoint id");
    let mut signatures = Vec::new();
    for state in members {
        assert_eq!(
            latest_scheduled(state).await.as_ref(),
            Some(&checkpoint),
            "members capture identical scheduled checkpoints"
        );
        let own = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        signatures.extend(
            state
                .mint
                .localstore()
                .get_federation_checkpoint_signatures(checkpoint_id)
                .await
                .expect("checkpoint signatures")
                .into_iter()
                .filter(|signature| signature.signer == own),
        );
    }
    for state in members {
        let own = state
            .mint
            .federation_config()
            .expect("federation config")
            .member_id();
        for signature in signatures
            .iter()
            .filter(|signature| signature.signer != own)
        {
            state
                .federation
                .persist_checkpoint_signature(checkpoint_id, signature)
                .await
                .expect("persist peer checkpoint signature");
        }
        assert_eq!(trusted_checkpoint_id(state).await, Some(checkpoint_id));
    }
    members[0]
        .mint
        .localstore()
        .get_federation_checkpoint_quorum(checkpoint_id)
        .await
        .expect("checkpoint quorum lookup")
        .expect("checkpoint quorum formed")
}

/// Order one mint quote per session in `sessions` on every member and promote
/// each scheduled checkpoint as it is captured.
async fn order_and_promote(
    members: &[&MintState],
    sessions: &[u64],
) -> (Vec<FederationOperationId>, Vec<FederationCheckpointQuorum>) {
    let mut operations = Vec::with_capacity(sessions.len());
    let mut quorums = Vec::new();
    for session in sessions {
        let before = latest_scheduled(members[0]).await;
        operations.push(order_quote(members, *session).await);
        if latest_scheduled(members[0]).await != before {
            quorums.push(promote_latest_scheduled(members).await);
        }
    }
    (operations, quorums)
}

fn router_for(state: &MintState) -> Router {
    Router::new()
        .nest("/api/federation/v1", create_federation_router())
        .with_state(state.clone())
}

/// History below the floor is deleted, while the floor checkpoint, the
/// trusted checkpoint and the validity window of the latest scheduled
/// checkpoint stay servable. Requests below the floor get 410 GONE, later
/// checkpoints still capture and agree, and pruning again deletes nothing.
#[tokio::test]
async fn pruning_keeps_the_floor_checkpoint_and_the_validity_window() {
    let (mut public_config, first_private, second_private) = transport_config();
    public_config.checkpoint_policy = retention_policy();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let members = [&first, &second];

    let (_, quorums) =
        order_and_promote(&members, &spaced_sessions(0, 4 * CHECKPOINT_INTERVAL)).await;
    assert_eq!(quorums.len(), 4, "four scheduled checkpoints were promoted");
    let previous = &quorums[2].checkpoint;
    let latest = &quorums[3].checkpoint;
    let latest_id = latest.id().expect("latest checkpoint id");

    let floor = retention_floor(&first).await.expect("retention floor");
    for state in members {
        let db = state.mint.localstore();
        assert_eq!(retention_floor(state).await, Some(floor), "members agree");
        // The validity window of the latest checkpoint starts at the previous
        // one, so the floor is exactly the previous checkpoint.
        assert_eq!(
            floor.checkpoint_id,
            previous.id().expect("previous checkpoint id")
        );
        assert_eq!(floor.journal_start, previous.journal_next_index);
        assert_eq!(floor.item_start, previous.finalized_next_item_index);
        assert!(floor.journal_start > 0 && floor.item_start > 0);
        let window = db
            .get_federation_state_transfer_window_bounds(latest)
            .await
            .expect("validity window bounds of the latest checkpoint");
        assert!(floor.journal_start <= window.journal_start);
        assert!(floor.item_start <= window.item_start);

        // Nothing below the floor is left; everything from it is.
        assert_eq!(
            journal_indexes(state).await,
            (floor.journal_start..latest.journal_next_index).collect::<Vec<_>>()
        );
        assert_eq!(
            item_indexes(state).await,
            (floor.item_start..latest.finalized_next_item_index).collect::<Vec<_>>()
        );
        let entries = db
            .get_federation_journal_entries()
            .await
            .expect("retained journal");
        assert_eq!(
            entries[0].previous_order_digest, floor.journal_base_digest,
            "the floor records the order digest just below it"
        );

        // Scheduled checkpoints below the floor checkpoint are deleted; the
        // floor checkpoint, the trusted one and their quorums stay.
        for quorum in &quorums[..2] {
            let checkpoint_id = quorum.checkpoint.id().expect("pruned checkpoint id");
            assert!(db
                .get_federation_checkpoint(checkpoint_id)
                .await
                .expect("pruned checkpoint lookup")
                .is_none());
            assert!(db
                .get_federation_checkpoint_quorum(checkpoint_id)
                .await
                .expect("pruned quorum lookup")
                .is_none());
        }
        for quorum in &quorums[2..] {
            let checkpoint_id = quorum.checkpoint.id().expect("kept checkpoint id");
            assert!(db
                .get_federation_checkpoint(checkpoint_id)
                .await
                .expect("kept checkpoint lookup")
                .is_some());
            assert_eq!(
                db.get_federation_checkpoint_quorum(checkpoint_id)
                    .await
                    .expect("kept quorum lookup")
                    .as_ref(),
                Some(quorum)
            );
        }
        assert_eq!(trusted_checkpoint_id(state).await, Some(latest_id));

        // Promotion already pruned; pruning again deletes nothing.
        assert_eq!(
            state
                .federation
                .prune_history_below_retention_floor()
                .await
                .expect("prune again"),
            0
        );
    }

    // The member serves its history from the floor up and answers GONE below.
    let first_member = public_config
        .member(first_private.member_id)
        .expect("first member")
        .clone();
    let refused = post_journal(
        State(first.clone()),
        Json(signed_catch_up_request(
            &public_config,
            &second_private,
            first_private.member_id,
            1,
            FederationJournalCatchUpRequest::new(0)
                .with_target_checkpoint(latest_id)
                .with_aleph_bft_next_item_index(0),
        )),
    )
    .await
    .expect_err("catch-up from genesis lies below the floor");
    assert_eq!(refused.status(), StatusCode::GONE);

    let client = FederationRestPeerClient::new_with_config(
        RouterFederationTransport::new(router_for(&first)),
        &public_config,
        second_private.clone(),
        FederationPeerSyncConfig::default(),
    )
    .expect("peer client");
    let page = client
        .catch_up_to_checkpoint(
            &first_member,
            &FederationJournalCatchUpRequest::new(floor.journal_start)
                .with_target_checkpoint(latest_id)
                .with_known_order_digest(floor.journal_base_digest)
                .with_aleph_bft_next_item_index(floor.item_start),
            latest,
        )
        .await
        .expect("catch-up from the floor");
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        (floor.journal_start..latest.journal_next_index).collect::<Vec<_>>()
    );
    assert_eq!(
        page.aleph_bft_finalized_items.len() as u64,
        latest.finalized_next_item_index - floor.item_start
    );
    let below = client
        .catch_up_to_checkpoint(
            &first_member,
            &FederationJournalCatchUpRequest::new(floor.journal_start - 1)
                .with_target_checkpoint(latest_id)
                .with_aleph_bft_next_item_index(floor.item_start - 1),
            latest,
        )
        .await
        .expect_err("catch-up below the floor");
    assert!(is_peer_history_pruned(&below), "unexpected error: {below}");

    // The validity window of the latest checkpoint is still complete; the
    // previous checkpoint's window reaches below the floor.
    let window = client
        .download_state_transfer_window_from_member(&first_member, &quorums[3])
        .await
        .expect("validity window of the latest checkpoint");
    assert!(window.bounds.journal_start >= floor.journal_start);
    assert!(window.bounds.item_start >= floor.item_start);
    assert_eq!(
        window.entries.len() as u64,
        latest.journal_next_index - window.bounds.journal_start
    );
    assert_eq!(
        window.items.len() as u64,
        latest.finalized_next_item_index - window.bounds.item_start
    );
    let rows = client
        .download_state_snapshot_from_member(&first_member, &quorums[3])
        .await
        .expect("snapshot of the latest checkpoint");
    // A member with empty federation state verifies and installs it.
    let fresh =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    fresh
        .mint
        .install_federation_state_transfer(&quorums[3], &rows, window)
        .await
        .expect("the served window installs");
    let older = client
        .download_state_transfer_window_from_member(&first_member, &quorums[2])
        .await
        .expect_err("validity window reaching below the floor");
    assert!(is_peer_history_pruned(&older), "unexpected error: {older}");

    // Two more operations in consecutive sessions: the next checkpoint is
    // captured on the pruned history, and its validity window reaches back
    // past the previous checkpoint, so it holds the floor where it is.
    let next_session = current_session(&first).await + 1;
    let (_, more) = order_and_promote(&members, &[next_session, next_session + 1]).await;
    assert_eq!(more.len(), 1);
    let newest = &more[0].checkpoint;
    for state in members {
        assert_eq!(retention_floor(state).await, Some(floor));
        let window = state
            .mint
            .localstore()
            .get_federation_state_transfer_window_bounds(newest)
            .await
            .expect("validity window bounds of the newest checkpoint");
        assert!(window.item_start < latest.finalized_next_item_index);
        assert!(floor.item_start <= window.item_start);
        assert_eq!(live_digest(state).await, newest.materialized_state_digest);
        assert_eq!(
            state
                .federation
                .prune_history_below_retention_floor()
                .await
                .expect("prune after the newest checkpoint"),
            0
        );
    }
    let first_frontier = frontier(&first).await;
    let second_frontier = frontier(&second).await;
    assert_eq!(first_frontier, second_frontier);
    assert_eq!(first_frontier.journal_next_index, newest.journal_next_index);
}

/// The journal entry of an unfinished melt saga holds the floor below it
/// until the saga ends; then the floor advances.
#[tokio::test]
async fn unfinished_melt_saga_pins_the_retention_floor() {
    let (mut public_config, first_private, second_private) = transport_config();
    public_config.checkpoint_policy = retention_policy();
    let first = federation_route_state_for_private(public_config.clone(), first_private).await;
    let second = federation_route_state_for_private(public_config, second_private).await;
    let members = [&first, &second];

    let (operations, _) = order_and_promote(&members, &spaced_sessions(0, 2)).await;
    // The second operation stands in for the melt operation the saga runs.
    let pinned_operation = operations[1];
    let saga = Saga {
        federation_operation_id: Some(pinned_operation.to_string()),
        ..Saga::new_melt(
            uuid::Uuid::new_v4(),
            MeltSagaState::PaymentPending,
            QuoteId::new().to_string(),
        )
    };
    let mut tx = first
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin saga transaction");
    tx.add_saga(&saga).await.expect("add saga");
    tx.commit().await.expect("commit saga");

    let (_, quorums) = order_and_promote(&members, &spaced_sessions(2, 6)).await;
    assert!(
        quorums.len() >= 3,
        "the natural floor is above the pinned entry"
    );
    let pinned = retention_floor(&first).await.expect("pinned floor");
    assert_eq!(pinned.journal_start, 1, "the saga's journal entry is kept");
    assert_eq!(pinned.item_start, 1);
    assert_eq!(journal_indexes(&first).await[0], 1);
    let unpinned = retention_floor(&second).await.expect("unpinned floor");
    assert!(unpinned.journal_start > pinned.journal_start);

    let mut tx = first
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin saga removal");
    tx.delete_saga(&saga.operation_id)
        .await
        .expect("delete saga");
    tx.commit().await.expect("commit saga removal");
    assert!(
        first
            .federation
            .prune_history_below_retention_floor()
            .await
            .expect("prune after the saga ended")
            > 0
    );
    assert_eq!(retention_floor(&first).await, Some(unpinned));
    assert_eq!(
        journal_indexes(&first).await,
        journal_indexes(&second).await
    );
}

/// A member with only key material, below every peer's floor, installs a
/// peer's latest scheduled checkpoint state from ordinary sync rounds, then
/// catches up later checkpoints and ends at the peers' tip and state.
#[tokio::test]
async fn member_below_every_peer_floor_installs_a_transfer_then_catches_up() {
    let (mut public_config, first_private, second_private, third_private) =
        transport_config_three();
    public_config.checkpoint_policy = retention_policy();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let third =
        federation_route_state_for_private(public_config.clone(), third_private.clone()).await;
    let peer_states = [&first, &second];

    let (_, quorums) =
        order_and_promote(&peer_states, &spaced_sessions(0, 4 * CHECKPOINT_INTERVAL)).await;
    let latest = &quorums.last().expect("latest quorum").checkpoint;
    let latest_id = latest.id().expect("latest checkpoint id");
    for state in peer_states {
        let floor = retention_floor(state).await.expect("peer floor");
        assert!(
            floor.journal_start > 0,
            "peers deleted their oldest history"
        );
    }

    let transport = RouterFederationTransportRegistry::default();
    for (state, private) in [(&first, &first_private), (&second, &second_private)] {
        transport.insert(
            public_config
                .member(private.member_id)
                .expect("peer member")
                .federation_api_url
                .clone(),
            router_for(state),
        );
    }
    let client = FederationRestPeerClient::new_with_config(
        transport,
        &public_config,
        third_private.clone(),
        FederationPeerSyncConfig::default(),
    )
    .expect("lagging member client");
    let peers = public_config
        .members
        .values()
        .filter(|member| member.member_id != third_private.member_id)
        .cloned()
        .collect::<Vec<_>>();

    // Every peer refuses to replay from genesis.
    for peer in &peers {
        let err = client
            .catch_up_to_checkpoint(
                peer,
                &FederationJournalCatchUpRequest::new(0)
                    .with_target_checkpoint(latest_id)
                    .with_aleph_bft_next_item_index(0),
                latest,
            )
            .await
            .expect_err("history from genesis is pruned");
        assert!(is_peer_history_pruned(&err), "unexpected error: {err}");
    }
    assert_eq!(
        frontier(&third).await,
        FederationCheckpointFrontier::genesis()
    );

    // An ordinary sync round installs the latest scheduled checkpoint state.
    run_federation_peer_sync_round(&third, &client, &peers).await;
    let installed = frontier(&third).await;
    assert_eq!(installed.trusted_checkpoint_id, Some(latest_id));
    assert_eq!(installed.journal_next_index, latest.journal_next_index);
    assert_eq!(installed.journal_order_digest, latest.journal_order_digest);
    assert_eq!(live_digest(&third).await, latest.materialized_state_digest);
    let third_floor = retention_floor(&third).await.expect("installed floor");
    assert!(third_floor.journal_start > 0);

    // The peers move on; following rounds catch up the suffix.
    let next_op = latest.journal_next_index;
    let (_, more) =
        order_and_promote(&peer_states, &spaced_sessions(next_op, CHECKPOINT_INTERVAL)).await;
    let newest = &more.last().expect("newest quorum").checkpoint;
    let newest_id = newest.id().expect("newest checkpoint id");
    for _ in 0..4 {
        run_federation_peer_sync_round(&third, &client, &peers).await;
        if frontier(&third).await.journal_next_index == newest.journal_next_index {
            break;
        }
    }
    let peer_frontier = frontier(&first).await;
    let caught_up = frontier(&third).await;
    assert_eq!(
        caught_up.journal_next_index,
        peer_frontier.journal_next_index
    );
    assert_eq!(
        caught_up.journal_order_digest,
        peer_frontier.journal_order_digest
    );
    assert_eq!(
        caught_up.application_digest,
        peer_frontier.application_digest
    );
    assert_eq!(
        caught_up.finalized_item_digest,
        peer_frontier.finalized_item_digest
    );
    assert_eq!(trusted_checkpoint_id(&third).await, Some(newest_id));
    assert_eq!(live_digest(&third).await, live_digest(&first).await);
    assert_eq!(live_digest(&third).await, newest.materialized_state_digest);
}

/// Order each envelope, authored by the first member, in its session on
/// every member, and promote each scheduled checkpoint as it is captured.
async fn order_envelopes_and_promote(
    members: &[&MintState],
    envelopes: Vec<(u64, FederationOperationEnvelope)>,
) -> (Vec<FederationOperationId>, Vec<FederationCheckpointQuorum>) {
    let mut operations = Vec::with_capacity(envelopes.len());
    let mut quorums = Vec::new();
    for (session, envelope) in envelopes {
        let before = latest_scheduled(members[0]).await;
        operations.push(order_envelope(members, session, envelope).await);
        if latest_scheduled(members[0]).await != before {
            quorums.push(promote_latest_scheduled(members).await);
        }
    }
    (operations, quorums)
}

async fn mint_payment_history(
    state: &MintState,
    quote: &QuoteId,
) -> Vec<cdk_common::database::mint::FederationStoredPaymentObservation> {
    state
        .mint
        .localstore()
        .get_federation_payment_observation_history(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("payment observation history")
}

async fn digest_at(
    state: &MintState,
    floor: u64,
) -> cdk::federation::FederationMaterializedStateDigest {
    state
        .mint
        .federation_materialized_state_snapshot_at(floor)
        .await
        .expect("materialized state")
        .digest()
        .expect("materialized digest")
}

/// A Bolt11 quote's payment observation retires when the quote is minted.
/// Two members delete it once it is below their retention floor; a third
/// keeps it. All three capture identical checkpoints, pass the audit of
/// their trusted checkpoint, and decide a replay of the retired observation
/// alike; a member installing a state transfer from a member that deleted
/// the observation reaches the signed digest.
#[tokio::test]
async fn retired_payment_observations_digest_alike_whether_or_not_deleted() {
    let (mut public_config, first_private, second_private, third_private) =
        transport_config_three();
    public_config.checkpoint_policy = retention_policy();
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let third =
        federation_route_state_for_private(public_config.clone(), third_private.clone()).await;
    third.federation.keep_terminal_entities(true);
    let members = [&first, &second, &third];

    let quote = QuoteId::new();
    let paid = paid_observation_envelope(&first, &quote);
    let paid_id = paid.operation_id().expect("paid observation id");
    let (mint, _) = mint_operation_envelope(&first, &quote, b"retention mint output");
    let sessions = spaced_sessions(0, 3);
    let (operations, _) = order_envelopes_and_promote(
        &members,
        vec![
            (sessions[0], mint_quote_envelope(&first, &quote)),
            (sessions[1], paid.clone()),
            (sessions[2], mint),
        ],
    )
    .await;
    assert_eq!(operations[1], paid_id);
    let mint_index = 2;
    for state in members {
        let history = mint_payment_history(state, &quote).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].record.operation_id, paid_id);
        assert_eq!(
            history[0].terminal_operation_index,
            Some(mint_index),
            "the mint that made the quote final retired its observation"
        );
        assert!(state
            .mint
            .localstore()
            .get_federation_payment_observations(
                FederationPaymentObservationContext::MintQuote,
                &quote.to_string(),
            )
            .await
            .expect("live observations")
            .is_empty());
    }

    // More operations move the floor past the mint. Every promotion checks
    // that the three members captured identical checkpoints.
    let (_, quorums) =
        order_and_promote(&members, &spaced_sessions(3, 4 * CHECKPOINT_INTERVAL - 1)).await;
    let latest = &quorums.last().expect("latest quorum").checkpoint;
    assert_eq!(latest.journal_next_index, 4 * CHECKPOINT_INTERVAL + 2);
    for state in members {
        let floor = retention_floor(state).await.expect("retention floor");
        assert!(floor.journal_start > mint_index);
    }
    assert!(mint_payment_history(&first, &quote).await.is_empty());
    assert!(mint_payment_history(&second, &quote).await.is_empty());
    assert_eq!(mint_payment_history(&third, &quote).await.len(), 1);
    assert_ne!(
        digest_at(&first, 0).await,
        digest_at(&third, 0).await,
        "the stored rows differ"
    );
    for state in members {
        state
            .federation
            .audit_trusted_checkpoint_materialized_state()
            .await
            .expect("audit of the trusted checkpoint");
    }

    // A replay of the retired observation, re-stamped by its author after its
    // validity window, is applied as a no-op on every member: nothing is
    // recorded, deleted or not.
    let replay_session = current_session(&first).await + FEDERATION_OPERATION_VALIDITY_SESSIONS + 1;
    let replay_id = order_envelope(&members, replay_session, paid).await;
    assert_eq!(replay_id, paid_id);
    for state in members {
        let entry = state
            .mint
            .localstore()
            .get_federation_journal_entry_by_operation_id(paid_id)
            .await
            .expect("replay lookup")
            .expect("replay entry");
        assert_eq!(entry.status(), FederationOperationStatus::Applied);
        let stored = state
            .mint
            .localstore()
            .get_mint_quote(&quote)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(stored.state(), MintQuoteState::Issued);
        assert_eq!(stored.amount_paid().value(), 1);
    }
    assert!(mint_payment_history(&first, &quote).await.is_empty());
    assert_eq!(mint_payment_history(&third, &quote).await.len(), 1);
    let replayed = live_digest(&first).await;
    assert_eq!(live_digest(&second).await, replayed);
    assert_eq!(live_digest(&third).await, replayed);

    // One more checkpoint after the replay, then a state transfer from the
    // member that deleted the observation.
    let next_session = current_session(&first).await + 1;
    let (_, more) = order_and_promote(
        &members,
        &(next_session..next_session + CHECKPOINT_INTERVAL).collect::<Vec<_>>(),
    )
    .await;
    let quorum = more.last().expect("checkpoint after the replay");
    let first_member = public_config
        .member(first_private.member_id)
        .expect("first member")
        .clone();
    let third_member = public_config
        .member(third_private.member_id)
        .expect("third member")
        .clone();
    let from_first = FederationRestPeerClient::new_with_config(
        RouterFederationTransport::new(router_for(&first)),
        &public_config,
        second_private.clone(),
        FederationPeerSyncConfig::default(),
    )
    .expect("client of the first member");
    let from_third = FederationRestPeerClient::new_with_config(
        RouterFederationTransport::new(router_for(&third)),
        &public_config,
        second_private.clone(),
        FederationPeerSyncConfig::default(),
    )
    .expect("client of the third member");
    let rows = from_first
        .download_state_snapshot_from_member(&first_member, quorum)
        .await
        .expect("snapshot from the member that deleted");
    assert_eq!(
        rows,
        from_third
            .download_state_snapshot_from_member(&third_member, quorum)
            .await
            .expect("snapshot from the member that kept"),
        "both members froze the same rows"
    );
    let window = from_first
        .download_state_transfer_window_from_member(&first_member, quorum)
        .await
        .expect("validity window");
    let fresh =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    fresh
        .mint
        .install_federation_state_transfer(quorum, &rows, window)
        .await
        .expect("install the transfer");
    assert_eq!(
        live_digest(&fresh).await,
        quorum.checkpoint.materialized_state_digest
    );
    let installed = fresh
        .mint
        .localstore()
        .get_mint_quote(&quote)
        .await
        .expect("installed quote lookup")
        .expect("installed quote");
    assert_eq!(installed.state(), MintQuoteState::Issued);
    assert_eq!(installed.amount_paid().value(), 1);
    assert!(mint_payment_history(&fresh, &quote).await.is_empty());
}
