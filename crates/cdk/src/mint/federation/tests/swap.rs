use super::*;

#[tokio::test]
async fn apply_federation_swap_spends_old_proofs_and_signs_rotated_outputs() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let old_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| keyset.active)
        .expect("old active keyset")
        .clone();
    let mut next_keyset = runtime_config
        .public_config()
        .keysets
        .iter()
        .find(|keyset| !keyset.active)
        .expect("inactive rotation keyset")
        .clone();
    next_keyset.active = true;
    let rotation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::KeysetRotation(
            FederationKeysetRotationOperation {
                keyset: next_keyset.clone(),
            },
        )),
    );
    persist_and_apply_entry(&mint, &rotation_entry).await;

    let input = proof_for_keyset(&old_keyset, 2, "rotated swap input");
    let output = output_for_keyset(&next_keyset, b"rotated swap output");
    let swap = SwapRequest::new(vec![input.clone()], vec![output.clone()]);
    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let swap_entry = appended_entry(
        &mut journal,
        effective.swap_operation(swap).expect("swap operation"),
    );
    persist_and_apply_entry(&mint, &swap_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored signatures");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated swap blind signature");
    assert_eq!(signature.keyset_id, next_keyset.id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("stored federation swap signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, swap_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(swap_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        swap_entry.consensus_evidence
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_rejects_duplicate_inputs_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let input = proof_for_runtime(&runtime_config, "duplicate swap input");
    let input_y = input.y().expect("duplicate swap input y");
    let output = output_for_runtime(&runtime_config, b"duplicate input swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![input.clone(), input], vec![output.clone()]),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await,
        Err(Error::DuplicateInputs)
    ));

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &[input_y],
        std::slice::from_ref(&output),
        "duplicate inputs",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_rejects_duplicate_outputs_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_input = proof_for_runtime(&runtime_config, "duplicate-output swap first input");
    let second_input = proof_for_runtime(&runtime_config, "duplicate-output swap second input");
    let input_ys = [
        first_input.y().expect("duplicate-output first input y"),
        second_input.y().expect("duplicate-output second input y"),
    ];
    let output = output_for_runtime(&runtime_config, b"duplicate swap output");
    let outputs = vec![output.clone(), output.clone()];
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![first_input, second_input], outputs.clone()),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await,
        Err(Error::DuplicateOutputs)
    ));

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &input_ys,
        &outputs,
        "duplicate outputs",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_rejects_oversized_proof_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let oversized_secret =
        Secret::new("s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1));
    let input = proof_for_secret(
        &runtime_config.public_config().keysets[0],
        2,
        oversized_secret,
    );
    let input_y = input.y().expect("oversized proof input y");
    let output = output_for_runtime(&runtime_config, b"oversized proof swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![input], vec![output.clone()]),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await,
        Err(Error::ProofContentTooLarge {
            actual,
            max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
        }) if actual == crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1
    ));

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &[input_y],
        std::slice::from_ref(&output),
        "oversized proof",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_rejects_bad_proof_signature_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let mut input = proof_for_runtime(&runtime_config, "bad swap proof signature");
    let input_y = input.y().expect("bad proof signature input y");
    input.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid federation swap proof signature",
    ));
    let output = output_for_runtime(&runtime_config, b"bad proof signature swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![input], vec![output.clone()]),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await,
        Err(Error::DHKE(cdk_common::dhke::Error::TokenNotVerified))
    ));

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &[input_y],
        std::slice::from_ref(&output),
        "bad proof signature",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_rejects_p2pk_missing_witness_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (input, _signing_key) = p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigInputs);
    let input_y = input.y().expect("p2pk missing witness input y");
    let output = output_for_runtime(&runtime_config, b"p2pk missing witness swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(vec![input], vec![output.clone()]),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await
            .is_err(),
        "P2PK input without a witness must be rejected"
    );

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &[input_y],
        std::slice::from_ref(&output),
        "p2pk missing witness",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_rejects_unbalanced_outputs_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let inputs = vec![
        proof_for_runtime(&runtime_config, "unbalanced swap first input"),
        proof_for_runtime(&runtime_config, "unbalanced swap second input"),
    ];
    let input_ys = inputs.ys().expect("unbalanced swap input ys");
    let output = output_for_runtime(&runtime_config, b"unbalanced swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(inputs, vec![output.clone()]),
        })),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await,
        Err(Error::TransactionUnbalanced(2, 1, 0))
    ));

    assert_rejected_swap_left_no_materialization(
        &localstore,
        swap_entry.operation_id,
        &input_ys,
        std::slice::from_ref(&output),
        "unbalanced outputs",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_swap_persists_accepted_materialization_rows() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let input = proof_for_runtime(&runtime_config, "accepted materialized swap input");
    let input_y = input.y().expect("swap input y");
    let output = output_for_runtime(&runtime_config, b"accepted materialized swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("accepted materialization swap operation"),
    );
    persist_and_apply_entry(&mint, &swap_entry).await;

    let operation_uuid = super::super::federation_operation_uuid(swap_entry.operation_id);
    let completed_operation = localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup")
        .expect("completed operation");
    assert_eq!(
        completed_operation.kind(),
        cdk_common::mint::OperationKind::Swap
    );
    assert_eq!(completed_operation.total_issued(), Amount::from(1));
    assert_eq!(completed_operation.total_redeemed(), Amount::from(1));
    assert_eq!(completed_operation.fee_collected(), Amount::ZERO);
    assert_eq!(completed_operation.completed_at(), &Some(0));
    assert_eq!(completed_operation.payment_method(), None);

    assert_eq!(
        localstore
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state"),
        vec![Some(State::Spent)]
    );
    assert_eq!(
        localstore
            .get_proof_ys_by_operation_id(&operation_uuid)
            .await
            .expect("proof ys by operation id"),
        vec![input_y]
    );
    assert_eq!(
        localstore
            .get_blinded_secrets_by_operation_id(&operation_uuid)
            .await
            .expect("blinded secrets by operation id"),
        vec![output.blinded_secret]
    );

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored swap blind signature");
    let signature = signatures[0].as_ref().expect("accepted swap signature");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, output.keyset_id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("stored federation swap signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, swap_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, runtime_config.member_id());
    assert_eq!(share_records[0].keyset_id, output.keyset_id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(swap_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        swap_entry.consensus_evidence
    );

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_swap_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == swap_entry.operation_id)
        .expect("stored swap entry");
    assert_eq!(
        stored_swap_entry.status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn apply_federation_swap_rolls_back_late_failure_and_replays() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let input = proof_for_runtime(&runtime_config, "late failure swap input");
    let input_y = input.y().expect("swap input y");
    let output = output_for_runtime(&runtime_config, b"late failure swap output");
    let swap_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input], vec![output.clone()]))
            .expect("late failure swap operation"),
    );
    persist_journal_entry(&localstore, &swap_entry).await;

    crate::test_helpers::mint::set_fail_for("FEDERATION_SWAP_APPLY_BEFORE_STATUS");
    let result = mint
        .apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
        .await;
    crate::test_helpers::mint::clear_fail_for("FEDERATION_SWAP_APPLY_BEFORE_STATUS");
    assert!(matches!(result, Err(Error::Internal)));

    assert_eq!(
        localstore
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state after rollback"),
        vec![None]
    );
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup after rollback"),
        vec![None]
    );
    assert!(localstore
        .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
        .await
        .expect("share lookup after rollback")
        .is_empty());
    let operation_uuid = super::super::federation_operation_uuid(swap_entry.operation_id);
    assert!(localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup after rollback")
        .is_none());
    assert!(localstore
        .get_proof_ys_by_operation_id(&operation_uuid)
        .await
        .expect("proof operation lookup after rollback")
        .is_empty());
    assert!(localstore
        .get_blinded_secrets_by_operation_id(&operation_uuid)
        .await
        .expect("blinded-message lookup after rollback")
        .is_empty());
    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup after rollback");
    assert_eq!(
        stored_entries
            .iter()
            .find(|entry| entry.operation_id == swap_entry.operation_id)
            .expect("swap entry after rollback")
            .status(),
        FederationOperationStatus::Accepted
    );

    assert_eq!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&swap_entry))
            .await
            .expect("replay swap after rollback"),
        vec![swap_entry.operation_id]
    );
    assert_eq!(
        localstore
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state after replay"),
        vec![Some(State::Spent)]
    );
    assert!(localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup after replay")[0]
        .is_some());
    assert_eq!(
        localstore
            .get_federation_signature_shares_for_output(swap_entry.operation_id, 0)
            .await
            .expect("share lookup after replay")
            .len(),
        1
    );
    assert!(localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup after replay")
        .is_some());
}

#[tokio::test]
async fn apply_federation_swap_accepts_p2pk_locked_input() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (mut input, signing_key) =
        p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigInputs);
    input.sign_p2pk(signing_key).expect("p2pk input witness");
    let output = output_for_runtime(&runtime_config, b"p2pk federated swap output");

    let entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("p2pk swap operation"),
    );
    persist_and_apply_entry(&mint, &entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored p2pk swap signatures");
    assert!(
        signatures[0].is_some(),
        "valid P2PK-locked federated swap output must receive a member share"
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_accepts_htlc_locked_input() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let preimage = crate::util::hex::encode([42; 32]);
    let mut input =
        htlc_locked_proof_for_runtime(&runtime_config, &preimage, &signing_key, SigFlag::SigInputs);
    input.add_preimage(preimage);
    input.sign_p2pk(signing_key).expect("htlc input witness");
    let output = output_for_runtime(&runtime_config, b"htlc federated swap output");

    let entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(vec![input.clone()], vec![output.clone()]))
            .expect("htlc swap operation"),
    );
    persist_and_apply_entry(&mint, &entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored htlc swap signatures");
    assert!(
        signatures[0].is_some(),
        "valid HTLC-locked federated swap output must receive a member share"
    );

    let input_states = mint
        .check_state(&cdk_common::CheckStateRequest {
            ys: vec![input.y().expect("input y")],
        })
        .await
        .expect("input states");
    assert_eq!(input_states.states[0].state, State::Spent);
}

#[tokio::test]
async fn apply_federation_swap_rejects_sig_all_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (input, signing_key) = p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigAll);
    let signed_output = output_for_runtime(&runtime_config, b"sig-all signed swap output");
    let mutated_output = output_for_runtime(&runtime_config, b"sig-all mutated swap output");
    let mut swap = SwapRequest::new(vec![input], vec![signed_output.clone()]);
    swap.sign_sig_all(signing_key).expect("sig_all witness");
    *swap.outputs_mut() = vec![mutated_output.clone()];

    assert!(
        runtime_config
            .validate_federated_swap_submission(&swap)
            .is_err(),
        "SIG_ALL output mutation must be rejected before consensus admission"
    );

    let entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Swap(FederationSwapOperation {
            request: swap,
        })),
    );
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_federation_journal_entry(&entry)
        .await
        .expect("persist mutated sig_all swap");
    tx.commit().await.expect("commit mutated sig_all swap");

    assert!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&entry))
            .await
            .is_err(),
        "SIG_ALL witness signed for a different output set must be rejected"
    );

    let signatures = localstore
        .get_blind_signatures(&[signed_output.blinded_secret, mutated_output.blinded_secret])
        .await
        .expect("sig_all output signatures");
    assert_eq!(
        signatures,
        vec![None, None],
        "SIG_ALL output mutation must fail before any federation share is stored"
    );
}

#[tokio::test]
async fn apply_federation_swap_rejects_signed_output_reuse_before_spending_fresh_input() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_input = proof_for_runtime(&runtime_config, "signed-output first swap input");
    let reused_output =
        output_for_runtime(&runtime_config, b"signed output reused by another swap");
    let first_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(
                vec![first_input],
                vec![reused_output.clone()],
            ))
            .expect("first swap operation"),
    );
    persist_and_apply_entry(&mint, &first_entry).await;

    let fresh_input = proof_for_runtime(&runtime_config, "signed-output fresh swap input");
    let fresh_input_y = fresh_input.y().expect("fresh input y");
    let second_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(
                vec![fresh_input],
                vec![reused_output.clone()],
            ))
            .expect("second swap operation"),
    );
    assert_ne!(first_entry.operation_id, second_entry.operation_id);
    persist_journal_entry(&localstore, &second_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&second_entry))
            .await,
        Err(Error::BlindedMessageAlreadySigned)
    ));
    assert_eq!(
        localstore
            .get_proofs_states(&[fresh_input_y])
            .await
            .expect("fresh input state"),
        vec![None]
    );
    assert!(localstore
        .get_federation_signature_shares_for_output(second_entry.operation_id, 0)
        .await
        .expect("second operation share lookup")
        .is_empty());
    assert_eq!(
        localstore
            .get_federation_signature_shares_for_output(first_entry.operation_id, 0)
            .await
            .expect("first operation share lookup")
            .len(),
        1
    );
}

#[tokio::test]
async fn apply_federation_swap_rejects_sliding_window_input_reuse_attack() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let inputs = vec![
        proof_for_runtime(&runtime_config, "sliding-swap-input-a"),
        proof_for_runtime(&runtime_config, "sliding-swap-input-b"),
        proof_for_runtime(&runtime_config, "sliding-swap-input-c"),
    ];
    let input_ys = inputs.ys().expect("input ys");
    let output_a = output_for_runtime(&runtime_config, b"sliding-swap-output-a");
    let output_b = output_for_runtime(&runtime_config, b"sliding-swap-output-b");
    let output_c = output_for_runtime(&runtime_config, b"sliding-swap-output-c");
    let output_d = output_for_runtime(&runtime_config, b"sliding-swap-output-d");
    let output_e = output_for_runtime(&runtime_config, b"sliding-swap-output-e");
    let output_f = output_for_runtime(&runtime_config, b"sliding-swap-output-f");

    let first_entry = appended_entry(
        &mut journal,
        runtime_config
            .swap_operation(SwapRequest::new(
                inputs.clone(),
                vec![output_a.clone(), output_b.clone(), output_c.clone()],
            ))
            .expect("first swap operation"),
    );
    persist_and_apply_entry(&mint, &first_entry).await;

    mint.apply_federation_journal_entries(std::slice::from_ref(&first_entry))
        .await
        .expect("same swap replay remains idempotent");

    let signed_outputs = localstore
        .get_blind_signatures(&[
            output_a.blinded_secret,
            output_b.blinded_secret,
            output_c.blinded_secret,
        ])
        .await
        .expect("accepted swap signatures");
    assert_eq!(signed_outputs.iter().filter(|sig| sig.is_some()).count(), 3);

    for outputs in [
        vec![output_d.clone(), output_e.clone(), output_f.clone()],
        vec![output_b.clone(), output_c.clone(), output_d.clone()],
        vec![output_c.clone(), output_d.clone(), output_a.clone()],
        vec![output_d.clone(), output_a.clone(), output_b.clone()],
    ] {
        let entry = appended_entry(
            &mut journal,
            runtime_config
                .swap_operation(SwapRequest::new(inputs.clone(), outputs))
                .expect("conflicting swap operation"),
        );
        let mut tx = localstore.begin_transaction().await.expect("begin");
        tx.add_federation_journal_entry(&entry)
            .await
            .expect("persist conflicting swap");
        tx.commit().await.expect("commit conflicting swap");

        assert!(matches!(
            mint.apply_federation_journal_entries(std::slice::from_ref(&entry))
                .await,
            Err(cdk_common::Error::TokenAlreadySpent)
        ));
    }

    let unsigned_outputs = localstore
        .get_blind_signatures(&[
            output_d.blinded_secret,
            output_e.blinded_secret,
            output_f.blinded_secret,
        ])
        .await
        .expect("unsigned conflicting swap outputs");
    assert_eq!(
        unsigned_outputs,
        vec![None, None, None],
        "conflicting swap outputs must not receive member shares"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("input states"),
        vec![Some(State::Spent), Some(State::Spent), Some(State::Spent)]
    );
}
