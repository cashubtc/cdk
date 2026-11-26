use cdk_common::database::mint::{
    FederationCheckpointSelector, FederationIndexRange, FederationSignatureShareKey,
};

use super::*;

async fn live_digest(state: &MintState) -> cdk::federation::FederationMaterializedStateDigest {
    state
        .mint
        .federation_materialized_state_snapshot()
        .await
        .expect("materialized state")
        .digest()
        .expect("materialized digest")
}

async fn own_share(
    state: &MintState,
    operation_id: FederationOperationId,
    output_index: u32,
) -> cdk::federation::FederationSignatureShareRecord {
    let mut shares = state
        .mint
        .localstore()
        .get_federation_signature_shares(FederationSignatureShareKey::Output {
            operation_id,
            output_index,
        })
        .await
        .expect("signature share lookup");
    assert_eq!(shares.len(), 1, "each member stores exactly its own share");
    shares.remove(0)
}

async fn apply_to_both(
    first: &MintState,
    second: &MintState,
    envelope: FederationOperationEnvelope,
) {
    apply_identical_finalized_unit(
        first,
        second,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .await;
}

/// A member wiped of federation state (but keeping its key material)
/// installs a peer's snapshot of a scheduled checkpoint, reaches the signed
/// digest, catches up the journal suffix with the ordinary protocol and
/// serves NUT-09 restore for outputs signed on both sides of the checkpoint.
#[tokio::test]
async fn wiped_member_installs_checkpoint_state_then_catches_up_and_restores() {
    let (mut public_config, first_private, second_private, third_private) =
        transport_config_three();
    public_config.checkpoint_policy =
        FederationCheckpointPolicy::new(6, 6).expect("checkpoint policy");
    let first =
        federation_route_state_for_private(public_config.clone(), first_private.clone()).await;
    let second =
        federation_route_state_for_private(public_config.clone(), second_private.clone()).await;
    let config = first
        .mint
        .federation_config()
        .expect("federation config")
        .clone();

    // Six operations before the scheduled checkpoint: quote A minted, quote B
    // paid but not yet minted, and one swap.
    let quote_a = QuoteId::new();
    let quote_b = QuoteId::new();
    let (mint_a, output_a) = mint_operation_envelope(&first, &quote_a, b"transfer mint output a");
    let mint_a_id = mint_a.operation_id().expect("mint a id");
    let input_one = proof_for_amount(&first, 1, "transfer-swap-one");
    let input_one_y = input_one.y().expect("swap one input y");
    let output_one = swap_output(&first, "transfer-swap-one");
    let swap_one = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input_one],
            vec![output_one.clone()],
        ))
        .expect("swap one");
    let swap_one_id = swap_one.operation_id().expect("swap one id");
    let swap_one_envelope = swap_one.clone();
    for envelope in [
        mint_quote_envelope(&first, &quote_a),
        paid_observation_envelope(&first, &quote_a),
        mint_a,
        mint_quote_envelope(&first, &quote_b),
        paid_observation_envelope(&first, &quote_b),
        swap_one,
    ] {
        apply_to_both(&first, &second, envelope).await;
    }

    let scheduled = first
        .mint
        .localstore()
        .get_federation_checkpoint(FederationCheckpointSelector::Latest(
            FederationCheckpointKind::Scheduled,
        ))
        .await
        .expect("scheduled checkpoint lookup")
        .expect("scheduled checkpoint");
    assert_eq!(scheduled.journal_next_index, 6);
    let scheduled_id = scheduled.id().expect("scheduled checkpoint id");
    let mut signatures = Vec::new();
    for state in [&first, &second] {
        signatures.extend(
            state
                .mint
                .localstore()
                .get_federation_checkpoint_signatures(scheduled_id)
                .await
                .expect("scheduled checkpoint signatures"),
        );
    }
    let scheduled_quorum = FederationCheckpointQuorum::new(scheduled.clone(), signatures);
    scheduled_quorum
        .verify(&public_config)
        .expect("scheduled checkpoint quorum");

    // Capture froze the rows that hash to the signed digest.
    let snapshots = first
        .mint
        .localstore()
        .get_federation_state_snapshots()
        .await
        .expect("stored snapshots");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].checkpoint_id, scheduled_id);
    assert_eq!(
        snapshots[0].materialized_state_digest,
        scheduled.materialized_state_digest
    );

    // Two operations after the checkpoint: the mint of quote B (created below
    // the checkpoint) and a second swap.
    let (mint_b, output_b) = mint_operation_envelope(&first, &quote_b, b"transfer mint output b");
    let mint_b_id = mint_b.operation_id().expect("mint b id");
    let input_two = proof_for_amount(&first, 1, "transfer-swap-two");
    let input_two_y = input_two.y().expect("swap two input y");
    let output_two = swap_output(&first, "transfer-swap-two");
    let swap_two = config
        .swap_operation(cdk::nuts::SwapRequest::new(
            vec![input_two],
            vec![output_two.clone()],
        ))
        .expect("swap two");
    let swap_two_id = swap_two.operation_id().expect("swap two id");
    for envelope in [mint_b, swap_two] {
        apply_to_both(&first, &second, envelope).await;
    }
    let (recovery, recovery_quorum) = recovery_checkpoint_quorum_for_state(
        &first,
        &public_config,
        Some(&scheduled),
        &[&first_private, &second_private],
    )
    .await;
    assert_eq!(recovery.journal_next_index, 8);
    let mut tx = first
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("store recovery quorum");
    tx.add_federation_checkpoint_quorum(&recovery_quorum)
        .await
        .expect("store recovery quorum");
    tx.commit().await.expect("commit recovery quorum");

    // The wiped member keeps only its key material.
    let mut third =
        federation_route_state_for_private(public_config.clone(), third_private.clone()).await;
    let first_member = public_config
        .member(first_private.member_id)
        .expect("first member");
    let client = FederationRestPeerClient::new_with_config(
        RouterFederationTransport::new(
            // The configured federation API URLs carry an `/api` prefix.
            Router::new()
                .nest("/api/federation/v1", create_federation_router())
                .with_state(first.clone()),
        ),
        &public_config,
        third_private.clone(),
        FederationPeerSyncConfig {
            catch_up_limit: 4,
            ..FederationPeerSyncConfig::default()
        },
    )
    .expect("transfer client");

    // Paged download; a tampered row or attachment is refused and leaves the
    // member empty.
    let rows = client
        .download_state_snapshot_from_member(first_member, &scheduled_quorum)
        .await
        .expect("download snapshot");
    assert_eq!(rows.len() as u64, snapshots[0].row_count);
    assert!(rows.len() > 4, "the download spans several pages");
    let window = client
        .download_state_transfer_window_from_member(first_member, &scheduled_quorum)
        .await
        .expect("download validity window");
    // Every operation before the checkpoint is in session 0, inside the window.
    assert_eq!(window.bounds.item_start, 0);
    assert_eq!(window.entries.len(), 6);
    assert_eq!(window.items.len(), 6);
    let mut tampered = rows.clone();
    let quote_row = tampered
        .iter_mut()
        .find(|row| row.row.table == "mint_quotes")
        .expect("mint quote row");
    quote_row.row.insert_field("amount_issued", "0");
    let refused = third
        .mint
        .install_federation_state_transfer(&scheduled_quorum, &tampered, window.clone())
        .await
        .expect_err("tampered row is refused");
    assert!(
        refused.to_string().contains("digest mismatch"),
        "unexpected refusal: {refused}"
    );
    let mut tampered = rows.clone();
    let observation_row = tampered
        .iter_mut()
        .find(|row| row.attachment.is_some())
        .expect("payment observation row");
    observation_row.attachment = Some(
        observation_row
            .attachment
            .clone()
            .expect("attachment")
            .replace("paid-", "forged-"),
    );
    let refused = third
        .mint
        .install_federation_state_transfer(&scheduled_quorum, &tampered, window.clone())
        .await
        .expect_err("tampered attachment is refused");
    assert!(
        refused.to_string().contains("attachment does not match"),
        "unexpected refusal: {refused}"
    );

    // A window whose application results were altered (here the swap, which
    // reserves no conflict keys) does not chain to the signed application
    // digest.
    let mut tampered_window = window.clone();
    tampered_window.entries[5].application =
        cdk::federation::FederationApplicationState::ConsensusRejected {};
    let refused = third
        .mint
        .install_federation_state_transfer(&scheduled_quorum, &rows, tampered_window)
        .await
        .expect_err("tampered window is refused");
    assert!(
        refused
            .to_string()
            .contains("application results do not chain"),
        "unexpected refusal: {refused}"
    );
    // A window cut short at its start, with base digests recomputed so every
    // chain still closes, does not reach back past the validity window.
    let mut truncated = window.clone();
    let first_item = truncated.items.remove(0);
    let first_entry = truncated.entries.remove(0);
    truncated.bounds = cdk::federation::FederationStateTransferWindowBounds {
        item_start: 1,
        journal_start: 1,
        item_base_digest: truncated
            .bounds
            .item_base_digest
            .append(&first_item)
            .expect("item base"),
        application_base_digest: truncated
            .bounds
            .application_base_digest
            .append_terminal(
                first_entry.index,
                first_entry.operation_id,
                &first_entry.application,
            )
            .expect("application base"),
    };
    let refused = third
        .mint
        .install_federation_state_transfer(&scheduled_quorum, &rows, truncated)
        .await
        .expect_err("truncated window is refused");
    assert!(
        refused
            .to_string()
            .contains("does not reach back past the validity window"),
        "unexpected refusal: {refused}"
    );
    assert!(third
        .mint
        .localstore()
        .get_federation_journal_entries(FederationIndexRange::ALL)
        .await
        .expect("journal after refused install")
        .is_empty());
    assert!(third
        .mint
        .localstore()
        .get_federation_materialized_state_source()
        .await
        .expect("state keys after refused install")
        .state_keys
        .is_empty());
    assert_eq!(
        third
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("frontier after refused install"),
        cdk::federation::FederationCheckpointFrontier::genesis()
    );

    client
        .transfer_state_from_member(&third.mint, first_member, &scheduled_quorum)
        .await
        .expect("install snapshot");
    assert_eq!(
        live_digest(&third).await,
        scheduled.materialized_state_digest
    );
    assert_eq!(
        third
            .mint
            .federation_state_snapshot_rows()
            .await
            .expect("installed rows"),
        rows
    );
    let frontier = third
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("installed frontier");
    assert_eq!(frontier.journal_next_index, 6);
    assert_eq!(frontier.trusted_checkpoint_id, Some(scheduled_id));
    // The validity window is stored below the checkpoint as history.
    assert_eq!(
        third
            .mint
            .localstore()
            .get_federation_journal_entries(FederationIndexRange::ALL)
            .await
            .expect("installed journal")
            .iter()
            .map(|entry| (entry.index, entry.operation_id))
            .collect::<Vec<_>>(),
        first
            .mint
            .localstore()
            .get_federation_journal_entries(FederationIndexRange::ALL)
            .await
            .expect("source journal")
            .iter()
            .take(6)
            .map(|entry| (entry.index, entry.operation_id))
            .collect::<Vec<_>>()
    );
    assert!(
        client
            .transfer_state_from_member(&third.mint, first_member, &scheduled_quorum)
            .await
            .is_err(),
        "a member that holds state does not install again"
    );

    // Restart from the installed checkpoint, then catch up the suffix.
    let (consensus, sequences) = crate::create_federation_runtime_state(&third.mint)
        .await
        .expect("restart from installed checkpoint");
    third.federation.member_mut_for_test().consensus = consensus.expect("configured member");
    third.federation.member_mut_for_test().sequences = sequences.expect("configured member");
    activate_local_test_finality(&mut third);
    client
        .catch_up_state_to_checkpoint_from_member(&third, first_member, &recovery_quorum)
        .await
        .expect("catch up suffix");

    let entries = third
        .mint
        .localstore()
        .get_federation_journal_entries(FederationIndexRange::ALL)
        .await
        .expect("caught-up journal");
    assert_eq!(
        entries
            .iter()
            .skip(6)
            .map(|entry| (entry.index, entry.operation_id))
            .collect::<Vec<_>>(),
        vec![(6, mint_b_id), (7, swap_two_id)]
    );
    assert!(entries
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));
    assert_eq!(live_digest(&third).await, live_digest(&first).await);
    assert_eq!(
        live_digest(&third).await,
        recovery.materialized_state_digest
    );

    // Restore and check-state on both sides of the checkpoint. The installed
    // member recomputed its own shares; combined with a peer's share they
    // give the same signature as any other pair of members.
    let threshold = usize::from(public_config.threshold_params.signature_threshold);
    for (operation_id, output) in [
        (mint_a_id, &output_a),
        (swap_one_id, &output_one),
        (mint_b_id, &output_b),
        (swap_two_id, &output_two),
    ] {
        let restored = third
            .mint
            .restore(RestoreRequest {
                outputs: vec![output.clone()],
            })
            .await
            .expect("restore on the installed member");
        assert_eq!(restored.outputs, vec![output.clone()]);
        assert_eq!(restored.signatures.len(), 1);

        let first_share = own_share(&first, operation_id, 0).await;
        let second_share = own_share(&second, operation_id, 0).await;
        let third_share = own_share(&third, operation_id, 0).await;
        assert_eq!(third_share.member_id, third_private.member_id);
        assert_eq!(third_share.blinded_secret, output.blinded_secret);
        assert_eq!(
            aggregate_blind_signature_shares(threshold, &[first_share.share, third_share.share])
                .expect("first and third aggregate"),
            aggregate_blind_signature_shares(threshold, &[first_share.share, second_share.share])
                .expect("first and second aggregate"),
        );
    }
    let checked = third
        .mint
        .check_state(&CheckStateRequest {
            ys: vec![input_one_y, input_two_y],
        })
        .await
        .expect("check-state on the installed member");
    assert!(checked
        .states
        .iter()
        .all(|state| state.state == cdk::nuts::State::Spent));
    let quote_b_state = third
        .mint
        .localstore()
        .get_mint_quote(&quote_b)
        .await
        .expect("quote b lookup")
        .expect("quote b");
    assert_eq!(quote_b_state.state(), MintQuoteState::Issued);

    // An operation from the validity window ordered again, and a new quote
    // that conflicts with one created in the window: the installed member
    // decides both exactly as its peers do, so all journals stay identical.
    let conflicting_quote = config.mint_quote_operation(
        quote_b.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(2),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_b.to_string(),
            request: format!("conflicting-invoice-{quote_b}"),
            amount: Some(Amount::from(2)),
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
    let payload = {
        let consensus = first
            .federation
            .consensus()
            .expect("first consensus")
            .lock()
            .await;
        let status = consensus.status();
        FederationAlephBftUnitPayload::new(
            status.federation_id,
            status.local_member,
            status.session,
            consensus.next_unit_index(),
            vec![
                FederationConsensusItem::operation(swap_one_envelope.clone()),
                FederationConsensusItem::operation(conflicting_quote),
            ],
        )
        .expect("replay payload")
    };
    for state in [&first, &second, &third] {
        state
            .federation
            .apply_finalized_unit(payload.clone())
            .await
            .expect("apply replayed unit");
    }
    let first_journal = first
        .mint
        .localstore()
        .get_federation_journal_entries(FederationIndexRange::ALL)
        .await
        .expect("first journal after replay");
    assert!(
        first_journal.len() > 8,
        "the conflicting quote is ordered after the checkpoint suffix"
    );
    assert!(
        first_journal
            .iter()
            .filter(|entry| entry.operation_id == swap_one_id)
            .count()
            == 1,
        "the window operation is a duplicate, not a new entry"
    );
    let outcome = |entries: Vec<cdk::federation::FederationJournalEntry>| {
        entries
            .into_iter()
            .map(|entry| {
                (
                    entry.index,
                    entry.operation_id,
                    serde_json::to_string(&entry.application).expect("application state"),
                )
            })
            .collect::<Vec<_>>()
    };
    let expected = outcome(first_journal);
    for state in [&second, &third] {
        let entries = state
            .mint
            .localstore()
            .get_federation_journal_entries(FederationIndexRange::ALL)
            .await
            .expect("journal after replay");
        let actual = outcome(entries);
        // The installed member holds no entries below its window, which here
        // starts at genesis, so all three journals must be equal.
        assert_eq!(actual, expected);
    }
    let first_frontier = first
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("first frontier after replay");
    for state in [&second, &third] {
        let frontier = state
            .mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("frontier after replay");
        assert_eq!(
            frontier.journal_next_index,
            first_frontier.journal_next_index
        );
        assert_eq!(
            frontier.journal_order_digest,
            first_frontier.journal_order_digest
        );
        assert_eq!(
            frontier.application_digest,
            first_frontier.application_digest
        );
        assert_eq!(
            frontier.finalized_item_digest,
            first_frontier.finalized_item_digest
        );
    }
    assert_eq!(live_digest(&third).await, live_digest(&first).await);
}

/// A member keeps the latest two scheduled snapshots and drops older ones.
#[tokio::test]
async fn scheduled_capture_keeps_the_latest_two_snapshots() {
    let (mut public_config, first_private, second_private) = transport_config();
    public_config.checkpoint_policy =
        FederationCheckpointPolicy::new(1, 1).expect("checkpoint policy");
    let first = federation_route_state_for_private(public_config.clone(), first_private).await;
    let second = federation_route_state_for_private(public_config, second_private).await;
    let mut captured = Vec::new();
    for _ in 0..3 {
        apply_to_both(
            &first,
            &second,
            mint_quote_envelope(&first, &QuoteId::new()),
        )
        .await;
        let checkpoint = first
            .mint
            .localstore()
            .get_federation_checkpoint(FederationCheckpointSelector::Latest(
                FederationCheckpointKind::Scheduled,
            ))
            .await
            .expect("scheduled checkpoint lookup")
            .expect("scheduled checkpoint");
        captured.push(checkpoint);
    }
    let snapshots = first
        .mint
        .localstore()
        .get_federation_state_snapshots()
        .await
        .expect("stored snapshots");
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.checkpoint_id)
            .collect::<Vec<_>>(),
        captured[1..]
            .iter()
            .map(|checkpoint| checkpoint.id().expect("checkpoint id"))
            .collect::<Vec<_>>()
    );
    for (snapshot, checkpoint) in snapshots.iter().zip(&captured[1..]) {
        assert_eq!(
            snapshot.materialized_state_digest,
            checkpoint.materialized_state_digest
        );
        let rows = first
            .mint
            .localstore()
            .get_federation_state_snapshot_rows(
                snapshot.checkpoint_id,
                0,
                snapshot.row_count as usize,
            )
            .await
            .expect("snapshot rows");
        assert_eq!(rows.len() as u64, snapshot.row_count);
        assert_eq!(
            cdk::federation::federation_state_snapshot_digest(&rows).expect("rows digest"),
            checkpoint.materialized_state_digest
        );
    }
    assert!(
        first
            .mint
            .localstore()
            .get_federation_state_snapshot_rows(
                captured[0].id().expect("pruned checkpoint id"),
                0,
                100,
            )
            .await
            .expect("pruned snapshot rows")
            .is_empty()
    );
}
