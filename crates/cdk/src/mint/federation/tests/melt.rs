use super::*;

#[tokio::test]
async fn process_federation_melt_change_signs_rotated_keyset() {
    let runtime_config = runtime_config_with_inactive_rotation_keyset();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
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

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let quote = QuoteId::new();
    let change_output = output_for_keyset(&next_keyset, b"rotated melt change");
    let change_secret = change_output.blinded_secret;
    let operation_id = effective
        .melt_operation(MeltRequest::new(
            quote.clone(),
            Vec::new(),
            Some(vec![change_output.clone()]),
        ))
        .expect("melt operation")
        .operation_id()
        .expect("operation id");
    let consensus_evidence =
        FederationJournalConsensusEvidence::new(7, 11, 13, effective.member_id(), 17);
    let signature_share_provenance =
        super::super::super::melt::shared::FederationSignatureShareProvenance::new(
            operation_id,
            23,
            Some(consensus_evidence),
        );

    let db = mint.localstore();
    let (change_sigs, tx) = super::super::super::melt::shared::process_federation_melt_change(
        &mint,
        &db,
        &quote,
        operation_id,
        signature_share_provenance,
        Amount::new(3, CurrencyUnit::Sat),
        Amount::new(2, CurrencyUnit::Sat),
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        vec![change_output],
    )
    .await
    .expect("process federated change");
    tx.commit().await.expect("commit change signatures");

    let signatures = localstore
        .get_blind_signatures(&[change_secret])
        .await
        .expect("stored change signature");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated melt change signature");
    assert_eq!(signature.keyset_id, next_keyset.id);
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(
        change_sigs.expect("change response")[0].keyset_id,
        next_keyset.id
    );

    let share_records = localstore
        .get_federation_signature_shares_for_blinded_message(change_secret)
        .await
        .expect("stored federation change signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, change_secret);
    assert_eq!(share_records[0].accepted_operation_index, Some(23));
    assert_eq!(
        share_records[0].consensus_evidence,
        Some(consensus_evidence)
    );
}

#[tokio::test]
async fn apply_federation_melt_rejects_bad_proof_signature_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let mut input = proof_for_runtime(&runtime_config, "bad melt proof signature");
    let input_y = input.y().expect("bad melt proof signature input y");
    input.c = PublicKey::from(BlsG1PublicKey::hash_to_curve(
        b"invalid federation melt proof signature",
    ));
    let mut change_output = output_for_runtime(&runtime_config, b"bad melt proof change");
    change_output.amount = Amount::ZERO;
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                quote.to_string(),
                vec![input],
                Some(vec![change_output.clone()]),
            ),
        })),
    );
    persist_journal_entry(&localstore, &melt_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::DHKE(cdk_common::dhke::Error::TokenNotVerified))
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        melt_entry.operation_id,
        &quote,
        &[input_y],
        std::slice::from_ref(&change_output),
        &make_payment_calls,
        "bad proof signature",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_oversized_proof_content_before_state_changes() {
    use cdk_common::nuts::{P2PKWitness, Witness};

    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let secret_quote = QuoteId::new();
    let secret_quote_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_quote_operation(secret_quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &secret_quote_entry).await;
    let keyset = &runtime_config.public_config().keysets[0];

    let oversized_secret =
        Secret::new("s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1));
    let secret_input = proof_for_secret(keyset, 2, oversized_secret);
    let secret_input_y = secret_input.y().expect("oversized secret melt input y");
    let secret_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(secret_quote.to_string(), vec![secret_input], None),
        })),
    );
    persist_journal_entry(&localstore, &secret_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&secret_entry))
            .await,
        Err(Error::ProofContentTooLarge {
            actual,
            max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
        }) if actual == crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        secret_entry.operation_id,
        &secret_quote,
        &[secret_input_y],
        &[],
        &make_payment_calls,
        "oversized secret",
    )
    .await;

    let witness_quote = QuoteId::new();
    let witness_quote_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_quote_operation(witness_quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &witness_quote_entry).await;

    let mut witness_input = proof_for_runtime(&runtime_config, "oversized witness melt input");
    witness_input.witness = Some(Witness::P2PKWitness(P2PKWitness {
        signatures: vec!["s".repeat(crate::mint::verification::MAX_PROOF_CONTENT_LEN + 1)],
    }));
    let witness_input_y = witness_input.y().expect("oversized witness melt input y");
    let witness_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(witness_quote.to_string(), vec![witness_input], None),
        })),
    );
    persist_journal_entry(&localstore, &witness_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&witness_entry))
            .await,
        Err(Error::ProofContentTooLarge {
            max: crate::mint::verification::MAX_PROOF_CONTENT_LEN,
            ..
        })
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        witness_entry.operation_id,
        &witness_quote,
        &[witness_input_y],
        &[],
        &make_payment_calls,
        "oversized witness",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_duplicate_inputs_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let input = proof_for_runtime(&runtime_config, "duplicate melt input");
    let input_y = input.y().expect("duplicate melt input y");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(quote.to_string(), vec![input.clone(), input], None),
        })),
    );
    persist_journal_entry(&localstore, &melt_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::DuplicateInputs)
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        melt_entry.operation_id,
        &quote,
        &[input_y],
        &[],
        &make_payment_calls,
        "duplicate inputs",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_p2pk_missing_witness_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let (input, _signing_key) = p2pk_locked_proof_for_runtime(&runtime_config, SigFlag::SigInputs);
    let input_y = input.y().expect("p2pk missing witness melt input y");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(quote.to_string(), vec![input], None),
        })),
    );
    persist_journal_entry(&localstore, &melt_entry).await;

    assert!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await
            .is_err(),
        "P2PK melt input without a witness must be rejected"
    );

    assert_rejected_melt_left_no_materialization(
        &localstore,
        melt_entry.operation_id,
        &quote,
        &[input_y],
        &[],
        &make_payment_calls,
        "p2pk missing witness",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_non_v3_change_before_state_changes() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let input = proof_for_runtime(&runtime_config, "non-v3 melt change input");
    let input_y = input.y().expect("non-v3 melt change input y");
    let mut change_output = non_v3_output_for_runtime(&runtime_config);
    change_output.amount = Amount::ZERO;
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                quote.to_string(),
                vec![input],
                Some(vec![change_output.clone()]),
            ),
        })),
    );
    persist_journal_entry(&localstore, &melt_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::Federation(
            FederationError::InvalidFederatedKeysetVersion { .. }
        ))
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        melt_entry.operation_id,
        &quote,
        &[input_y],
        std::slice::from_ref(&change_output),
        &make_payment_calls,
        "non-v3 change",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_unbalanced_inputs_before_payment_attempt() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let input = proof_for_runtime(&runtime_config, "unbalanced melt input");
    let input_y = input.y().expect("unbalanced melt input y");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(quote.clone(), vec![input], None))
            .expect("unbalanced melt operation"),
    );
    persist_journal_entry(&localstore, &melt_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::TransactionUnbalanced(1, 1, 1))
    ));

    assert_rejected_melt_left_no_materialization(
        &localstore,
        melt_entry.operation_id,
        &quote,
        &[input_y],
        &[],
        &make_payment_calls,
        "unbalanced inputs",
    )
    .await;
}

#[tokio::test]
async fn apply_federation_melt_rejects_pending_quote_reuse_before_payment_attempt() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, original_input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;
    let payment_calls_after_original = make_payment_calls.load(Ordering::Relaxed);

    let input = proof_for_runtime(&runtime_config, "pending quote reuse melt input");
    let input_y = input.y().expect("pending quote reuse melt input y");
    let conflicting_envelope = runtime_config
        .melt_operation(MeltRequest::new(quote.clone(), vec![input], None))
        .expect("pending quote reuse melt operation");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("pending quote reuse operation id");
    let conflicting_operation_id_string = conflicting_operation_id.to_string();
    let conflicting_entry = FederationJournalEntry {
        index: 42,
        operation_id: conflicting_operation_id,
        conflict_key: conflicting_envelope.operation.conflict_key(),
        conflict_keys: conflicting_envelope.operation.conflict_keys(),
        envelope: conflicting_envelope,
        previous_order_digest: FederationJournalDigest::from_bytes([1; 32]),
        order_digest: FederationJournalDigest::from_bytes([2; 32]),
        operation_outcome_hash: None,
        state_digest: None,
        consensus_evidence: None,
        status: FederationOperationStatus::Accepted,
    };
    persist_journal_entry(&localstore, &conflicting_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&conflicting_entry))
            .await,
        Err(Error::PendingQuote)
    ));

    assert_eq!(
        localstore
            .get_proofs_states(&[input_y])
            .await
            .expect("conflicting pending-quote proof state"),
        vec![None],
        "conflicting pending-quote melt must fail before proof state storage"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&original_input_ys)
            .await
            .expect("original pending-quote proof states"),
        vec![Some(State::Pending), Some(State::Pending)],
        "the original pending melt must remain reserved"
    );

    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("pending quote lookup")
        .expect("pending quote");
    assert_eq!(
        stored_quote.state,
        MeltQuoteState::Pending,
        "conflicting pending-quote melt must leave the first operation pending"
    );
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("pending quote saga lookup")
        .expect("pending quote saga");
    assert_ne!(
        saga.federation_operation_id.as_deref(),
        Some(conflicting_operation_id_string.as_str()),
        "conflicting operation must not take over the pending quote saga"
    );
    assert!(
        localstore
            .get_completed_operation(&super::super::federation_operation_uuid(
                conflicting_entry.operation_id
            ))
            .await
            .expect("conflicting pending-quote completed operation lookup")
            .is_none(),
        "conflicting pending-quote melt must fail before completed-operation storage"
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        payment_calls_after_original,
        "conflicting pending-quote melt must not attempt another payment"
    );
}

#[tokio::test]
async fn melt_federation_pending_quote_allows_same_operation_replay_without_payment_attempt() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "same operation replay first input"),
        proof_for_runtime(&runtime_config, "same operation replay second input"),
    ];
    let input_ys = proofs.ys().expect("same operation replay input ys");
    let request = MeltRequest::new(quote.clone(), proofs, None);
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(request.clone())
            .expect("same operation replay melt operation"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;
    let payment_calls_after_original = make_payment_calls.load(Ordering::Relaxed);

    let replay = mint
        .melt_federation(&request, melt_entry.operation_id)
        .await
        .expect("same operation pending replay");
    assert_eq!(replay.state(), MeltQuoteState::Pending);
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        payment_calls_after_original,
        "same-operation pending replay must not attempt another payment"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("same operation replay proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
}

#[tokio::test]
async fn apply_federation_melt_rejects_paid_quote_reuse_with_different_inputs() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, original_input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    apply_paid_melt_observation_quorum(
        &mint,
        &runtime_config,
        &mut journal,
        &quote,
        "terminal-paid-input-preimage",
    )
    .await;
    let payment_calls_after_paid = make_payment_calls.load(Ordering::Relaxed);

    let conflicting_input = proof_for_runtime(&runtime_config, "paid quote reuse different input");
    let conflicting_input_y = conflicting_input
        .y()
        .expect("paid quote reuse different input y");
    let conflicting_envelope = runtime_config
        .melt_operation(MeltRequest::new(
            quote.clone(),
            vec![conflicting_input],
            None,
        ))
        .expect("paid quote reuse different input operation");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("paid quote reuse different input operation id");
    let conflicting_entry = accepted_entry_without_journal(43, conflicting_envelope);
    persist_journal_entry(&localstore, &conflicting_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&conflicting_entry))
            .await,
        Err(Error::RequestAlreadyPaid)
    ));

    assert_eq!(
        localstore
            .get_proofs_states(&[conflicting_input_y])
            .await
            .expect("conflicting terminal-paid input state"),
        vec![None],
        "conflicting terminal-paid melt must fail before proof state storage"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&original_input_ys)
            .await
            .expect("original terminal-paid input states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert!(
        localstore
            .get_completed_operation(&super::super::federation_operation_uuid(
                conflicting_operation_id
            ))
            .await
            .expect("conflicting terminal-paid completed operation lookup")
            .is_none(),
        "conflicting terminal-paid melt must fail before completed-operation storage"
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(conflicting_operation_id)
            .await
            .expect("conflicting terminal-paid journal lookup")
            .expect("conflicting terminal-paid journal entry")
            .status,
        FederationOperationStatus::Accepted,
        "conflicting terminal-paid melt must not be marked applied"
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        payment_calls_after_paid,
        "conflicting terminal-paid melt must not attempt another payment"
    );
}

#[tokio::test]
async fn apply_federation_melt_rejects_paid_quote_reuse_with_different_change_outputs() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let original_proofs = vec![
        proof_for_runtime(&runtime_config, "terminal-paid-change-input-one"),
        proof_for_runtime(&runtime_config, "terminal-paid-change-input-two"),
        proof_for_runtime(&runtime_config, "terminal-paid-change-input-three"),
    ];
    let original_input_ys = original_proofs.ys().expect("terminal-paid change input ys");
    let mut original_change = output_for_runtime(&runtime_config, b"terminal-paid original change");
    original_change.amount = Amount::ZERO;
    let original_request = MeltRequest::new(
        quote.clone(),
        original_proofs.clone(),
        Some(vec![original_change.clone()]),
    );
    let original_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(original_request.clone())
            .expect("terminal-paid change melt operation"),
    );
    persist_and_apply_entry(&mint, &original_entry).await;

    apply_paid_melt_observation_quorum(
        &mint,
        &runtime_config,
        &mut journal,
        &quote,
        "terminal-paid-change-preimage",
    )
    .await;
    let payment_calls_after_paid = make_payment_calls.load(Ordering::Relaxed);

    let mut conflicting_change =
        output_for_runtime(&runtime_config, b"terminal-paid conflicting change");
    conflicting_change.amount = Amount::ZERO;
    let conflicting_request = MeltRequest::new(
        quote.clone(),
        original_proofs,
        Some(vec![conflicting_change.clone()]),
    );
    let conflicting_envelope = runtime_config
        .melt_operation(conflicting_request)
        .expect("terminal-paid conflicting change operation");
    let conflicting_operation_id = conflicting_envelope
        .operation_id()
        .expect("terminal-paid conflicting change operation id");
    let conflicting_entry = accepted_entry_without_journal(44, conflicting_envelope);
    persist_journal_entry(&localstore, &conflicting_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&conflicting_entry))
            .await,
        Err(Error::RequestAlreadyPaid)
    ));

    assert_eq!(
        localstore
            .get_proofs_states(&original_input_ys)
            .await
            .expect("original terminal-paid change input states"),
        vec![Some(State::Spent), Some(State::Spent), Some(State::Spent)]
    );
    assert!(
        localstore
            .get_blind_signatures(&[conflicting_change.blinded_secret])
            .await
            .expect("conflicting terminal-paid change signature lookup")[0]
            .is_none(),
        "conflicting terminal-paid change output must remain unsigned"
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(conflicting_operation_id)
            .await
            .expect("conflicting terminal-paid change journal lookup")
            .expect("conflicting terminal-paid change journal entry")
            .status,
        FederationOperationStatus::Accepted,
        "conflicting terminal-paid change operation must not be marked applied"
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        payment_calls_after_paid,
        "conflicting terminal-paid change operation must not attempt another payment"
    );
}

#[tokio::test]
async fn apply_federation_melt_retries_failed_quote_state_with_payment_attempt() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    set_melt_quote_state(&localstore, &quote, MeltQuoteState::Failed).await;

    let inputs = vec![
        proof_for_runtime(&runtime_config, "failed quote retry input one"),
        proof_for_runtime(&runtime_config, "failed quote retry input two"),
    ];
    let input_ys = inputs.ys().expect("failed quote retry input ys");
    let request = MeltRequest::new(quote.clone(), inputs, None);
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(request)
            .expect("failed quote retry melt operation"),
    );

    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        1,
        "failed quote retry should attempt the accepted outgoing payment"
    );
    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("failed quote retry quote lookup")
        .expect("failed quote retry quote");
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("failed quote retry saga lookup")
        .expect("failed quote retry saga");
    let operation_id = melt_entry.operation_id.to_string();
    assert_eq!(
        saga.federation_operation_id.as_deref(),
        Some(operation_id.as_str())
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("failed quote retry proof state"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
    assert!(
        localstore
            .get_completed_operation(&super::super::federation_operation_uuid(
                melt_entry.operation_id
            ))
            .await
            .expect("failed quote retry completed operation lookup")
            .is_none(),
        "pending retry must not store a terminal completed operation"
    );
}

#[tokio::test]
async fn apply_federation_melt_keeps_quote_pending_when_backend_status_is_unknown() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let description = cdk_fake_wallet::FakeInvoiceDescription {
        pay_invoice_state: MeltQuoteState::Pending,
        check_payment_state: MeltQuoteState::Unknown,
        pay_err: false,
        check_err: false,
    };
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(
            quote.clone(),
            MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: cdk_fake_wallet::create_fake_invoice(
                    1_000,
                    serde_json::to_string(&description).expect("fake invoice description"),
                ),
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let inputs = vec![
        proof_for_runtime(&runtime_config, "unknown backend input one"),
        proof_for_runtime(&runtime_config, "unknown backend input two"),
    ];
    let input_ys = inputs.ys().expect("unknown backend input ys");
    let mut change_output = output_for_runtime(&runtime_config, b"unknown backend change");
    change_output.amount = Amount::ZERO;
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(
                quote.clone(),
                inputs,
                Some(vec![change_output.clone()]),
            ))
            .expect("unknown backend melt operation"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("unknown backend proof states"),
        vec![Some(State::Pending), Some(State::Pending)],
        "unknown backend payment status must leave proofs pending"
    );
    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("unknown backend quote lookup")
        .expect("unknown backend quote");
    assert_eq!(
        stored_quote.state,
        MeltQuoteState::Pending,
        "unknown backend payment status must keep the durable quote pending"
    );
    assert_ne!(
        stored_quote.state,
        MeltQuoteState::Unknown,
        "backend uncertainty must not be persisted as an UNKNOWN quote state"
    );
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("unknown backend saga lookup")
        .expect("unknown backend saga");
    assert_eq!(
        saga.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted)
    );
    let operation_id = melt_entry.operation_id.to_string();
    assert_eq!(
        saga.federation_operation_id.as_deref(),
        Some(operation_id.as_str())
    );
    assert!(
        localstore
            .get_completed_operation(&super::super::federation_operation_uuid(
                melt_entry.operation_id
            ))
            .await
            .expect("unknown backend completed operation lookup")
            .is_none(),
        "unknown backend payment status must not store a terminal completed operation"
    );
    assert_eq!(
        localstore
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("unknown backend change signature lookup"),
        vec![None],
        "unknown backend payment status must not sign change"
    );
    assert!(
        localstore
            .get_federation_signature_shares_for_blinded_message(change_output.blinded_secret)
            .await
            .expect("unknown backend change share lookup")
            .is_empty(),
        "unknown backend payment status must not persist change shares"
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(melt_entry.operation_id)
            .await
            .expect("unknown backend journal lookup")
            .expect("unknown backend journal entry")
            .status,
        FederationOperationStatus::Applied,
        "accepted unknown-backend melt should apply as a pending operation"
    );
}

#[tokio::test]
async fn apply_federation_melt_payment_observation_keeps_unknown_transient() {
    let runtime_config = runtime_config();
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let unknown_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Unknown,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &unknown_observation).await;

    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("unknown observation quote lookup")
        .expect("unknown observation quote");
    assert_eq!(
        stored_quote.state,
        MeltQuoteState::Pending,
        "unknown observations are backend status only and must not become durable quote state"
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("unknown observation saga lookup")
            .is_some(),
        "unknown observations must keep the melt parked for later polling"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("unknown observation proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
    assert!(
        localstore
            .get_completed_operation(&super::super::federation_operation_uuid(
                unknown_observation.operation_id
            ))
            .await
            .expect("unknown observation completed operation lookup")
            .is_none(),
        "unknown observations must not store terminal melt responses"
    );

    let observations = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("unknown observation rows");
    assert_eq!(observations.len(), 1);
    let observation = observations
        .first()
        .expect("stored unknown payment observation");
    match &observation.observation {
        FederationPaymentObservation::MeltQuote(stored) => {
            assert_eq!(stored.state, MeltQuoteState::Unknown);
        }
        FederationPaymentObservation::MintQuote(_) => panic!("expected melt observation"),
    }
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(unknown_observation.operation_id)
            .await
            .expect("unknown observation journal lookup")
            .expect("unknown observation journal entry")
            .status,
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn accepted_federation_melt_persists_reserve_before_payment_attempt() {
    let runtime_config = runtime_config();
    let (mint, localstore, expected_reserve, make_payment_calls) =
        federated_test_mint_with_reserve_checking_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "reserve-before-payment-input-one"),
        proof_for_runtime(&runtime_config, "reserve-before-payment-input-two"),
    ];
    let input_ys = proofs.ys().expect("reserve-before-payment input ys");
    *expected_reserve.lock().await = Some(ExpectedMeltReserve {
        quote: quote.clone(),
        input_ys: input_ys.clone(),
    });

    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(quote.clone(), proofs, None))
            .expect("melt operation"),
    );
    assert!(
        runtime_config.should_attempt_melt_payment(melt_entry.operation_id),
        "single-member federation should attempt the accepted external melt payment"
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);

    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("melt saga lookup")
        .expect("melt saga");
    assert_eq!(
        saga.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted)
    );
    assert_eq!(
        saga.federation_operation_id,
        Some(melt_entry.operation_id.to_string())
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal entries");
    let stored_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == melt_entry.operation_id)
        .expect("stored melt entry");
    assert_eq!(stored_entry.status, FederationOperationStatus::Applied);
}

#[tokio::test]
async fn real_policy_non_elected_melt_member_reserves_without_payment_attempt() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let operation_id = operation_id_with_selector(1);
    assert_eq!(
        runtime_config.elected_melt_payment_member(operation_id),
        MemberId::new(2).expect("member two")
    );
    assert!(!runtime_config.should_attempt_melt_payment(operation_id));

    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config.clone(), payment_processor).await;
    let quote = QuoteId::new();
    mint.get_federation_melt_quote(quote.clone(), pending_melt_quote_request(1_000))
        .await
        .expect("federation melt quote");
    let input_ys;
    let request = {
        let proofs = vec![
            proof_for_runtime(&runtime_config, "non-elected-melt-input-one"),
            proof_for_runtime(&runtime_config, "non-elected-melt-input-two"),
        ];
        input_ys = proofs.ys().expect("input ys");
        MeltRequest::new(quote.clone(), proofs, None)
    };

    let response = mint
        .melt_federation(&request, operation_id)
        .await
        .expect("non-elected federation melt reserve");

    assert_eq!(response.state(), MeltQuoteState::Pending);
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "non-elected member should keep pending saga for later payment observation"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
}

#[test]
fn melt_payment_actor_selection_is_deterministic_for_real_policies() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let real_policy_config = runtime_config_with_payment_policy(policy);
    let member_one_operation = operation_id_with_selector(0);
    let member_two_operation = operation_id_with_selector(1);

    assert_eq!(
        real_policy_config.elected_melt_payment_member(member_one_operation),
        MemberId::new(1).expect("member one")
    );
    assert!(real_policy_config.should_attempt_melt_payment(member_one_operation));
    assert_eq!(
        real_policy_config.elected_melt_payment_member(member_two_operation),
        MemberId::new(2).expect("member two")
    );
    assert!(!real_policy_config.should_attempt_melt_payment(member_two_operation));
    assert!(runtime_config().should_attempt_melt_payment(member_two_operation));
}

#[tokio::test]
async fn internal_melt_to_mint_settlement_updates_both_quotes_in_accepted_melt() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let invoice = cdk_fake_wallet::create_fake_invoice(1_000, "internal settlement".to_string());

    let mint_quote = QuoteId::new();
    let mint_quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            mint_quote.clone(),
            bolt11_quote_request(1),
            MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote: mint_quote.to_string(),
                request: invoice.to_string(),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                state: MintQuoteState::Unpaid,
                expiry: None,
                pubkey: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &mint_quote_entry).await;

    let melt_quote = QuoteId::new();
    let melt_quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(
            melt_quote.clone(),
            MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }),
        ),
    );
    persist_and_apply_entry(&mint, &melt_quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "internal-settlement-input-one"),
        proof_for_runtime(&runtime_config, "internal-settlement-input-two"),
    ];
    let input_ys = proofs.ys().expect("input ys");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(melt_quote.clone(), proofs, None))
            .expect("melt operation"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    let stored_mint_quote = localstore
        .get_mint_quote(&mint_quote)
        .await
        .expect("mint quote lookup")
        .expect("mint quote");
    assert_eq!(stored_mint_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_mint_quote.amount_paid(),
        Amount::new(1, CurrencyUnit::Sat)
    );
    assert_eq!(stored_mint_quote.payments.len(), 1);
    assert_eq!(
        stored_mint_quote.payments[0].payment_id,
        melt_quote.to_string()
    );
    assert_eq!(
        stored_mint_quote.payments[0].time,
        super::super::federation_accepted_logical_time(&melt_entry)
    );

    let stored_melt_quote = localstore
        .get_melt_quote(&melt_quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(stored_melt_quote.state, MeltQuoteState::Paid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&melt_quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "internal settlement should complete and delete the melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert!(
        localstore
            .get_federation_payment_observations(
                FederationPaymentObservationContext::MeltQuote,
                &melt_quote.to_string(),
            )
            .await
            .expect("melt payment observations")
            .is_empty(),
        "internal settlement should not require a separate melt payment observation"
    );
}

#[tokio::test]
async fn real_policy_melt_payment_observations_finalize_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let first_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let quote_after_single = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_single.state, MeltQuoteState::Pending);
    assert_eq!(quote_after_single.payment_proof, None);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "single real-backend observation must not finalize the saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after single observation"),
        vec![Some(State::Pending), Some(State::Pending)]
    );

    let observations_after_single = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("single melt observation rows");
    assert_eq!(observations_after_single.len(), 1);

    let matching_second = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let quote_after_quorum = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_quorum.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_quorum.payment_proof,
        Some("paid-preimage".to_string())
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "matching quorum should clear the pending melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after quorum"),
        vec![Some(State::Spent), Some(State::Spent)]
    );

    let conflicting_third = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 3,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("different-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &conflicting_third).await;

    let quote_after_conflict = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_conflict.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_conflict.payment_proof,
        Some("paid-preimage".to_string())
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting melt observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
    assert_eq!(
        observations_after_conflict[2].observation.observer(),
        MemberId::new(3).expect("member three")
    );
}

#[tokio::test]
async fn catch_up_melt_payment_observations_finalize_without_duplicate_payment() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), pending_melt_quote_request(1_000)),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let (request, input_ys, change_output, melt_entry) = (0..128)
        .find_map(|index| {
            let proofs = vec![
                proof_for_runtime(&runtime_config, &format!("catch-up-paid-input-{index}-one")),
                proof_for_runtime(&runtime_config, &format!("catch-up-paid-input-{index}-two")),
                proof_for_runtime(
                    &runtime_config,
                    &format!("catch-up-paid-input-{index}-three"),
                ),
            ];
            let input_ys = proofs.ys().expect("catch-up paid input ys");
            let mut change_output = output_for_runtime(
                &runtime_config,
                format!("catch-up-paid-change-{index}").as_bytes(),
            );
            change_output.amount = Amount::ZERO;
            let request =
                MeltRequest::new(quote.clone(), proofs, Some(vec![change_output.clone()]));
            let envelope = runtime_config
                .melt_operation(request.clone())
                .expect("melt operation envelope");
            let operation_id = envelope.operation_id().expect("melt operation id");
            if runtime_config.should_attempt_melt_payment(operation_id) {
                return None;
            }

            Some((
                request,
                input_ys,
                change_output,
                appended_entry(&mut journal, envelope),
            ))
        })
        .expect("non-local elected melt operation");

    assert!(
        !runtime_config.should_attempt_melt_payment(melt_entry.operation_id),
        "catch-up replay member must not be elected for the outgoing payment"
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "non-elected catch-up member must reserve without paying"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("reserved proof states"),
        vec![
            Some(State::Pending),
            Some(State::Pending),
            Some(State::Pending)
        ]
    );
    assert!(localstore
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("pre-observation change signature lookup")[0]
        .is_none());

    let first_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("catch-up-paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &first_observation).await;
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "first catch-up observation must not pay"
    );
    let quote_after_single = localstore
        .get_melt_quote(&quote)
        .await
        .expect("single-observation quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_single.state, MeltQuoteState::Pending);

    let matching_second = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 3,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("catch-up-paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "catch-up paid quorum must not duplicate the outgoing payment"
    );
    let quote_after_quorum = localstore
        .get_melt_quote(&quote)
        .await
        .expect("quorum quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_quorum.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_quorum.payment_proof,
        Some("catch-up-paid-preimage".to_string())
    );
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("quorum saga lookup")
            .is_none(),
        "paid catch-up quorum should clear the melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("quorum proof states"),
        vec![Some(State::Spent), Some(State::Spent), Some(State::Spent)]
    );
    let change = localstore
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("quorum change signature lookup")[0]
        .clone()
        .expect("catch-up paid change signature");
    assert_eq!(change.amount, Amount::from(1));

    let duplicate = mint
        .melt_federation(&request, melt_entry.operation_id)
        .await
        .expect("duplicate catch-up melt should return persisted final response");
    assert_eq!(duplicate.state(), MeltQuoteState::Paid);
    let duplicate_change = duplicate.change().expect("duplicate catch-up change");
    assert_eq!(duplicate_change.len(), 1);
    assert_eq!(duplicate_change[0].amount, Amount::from(1));
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "duplicate final response after catch-up must not pay"
    );
}

#[tokio::test]
async fn stale_pending_melt_observation_after_paid_settlement_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let paid_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("paid-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &paid_observation).await;

    let quote_after_paid = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_paid.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_paid.payment_proof,
        Some("paid-preimage".to_string())
    );

    let stale_pending = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Pending,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &stale_pending).await;

    let quote_after_stale = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_stale.state, MeltQuoteState::Paid);
    assert_eq!(
        quote_after_stale.payment_proof,
        Some("paid-preimage".to_string())
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after stale pending"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    let observations = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("observation rows");
    assert_eq!(observations.len(), 2);
}

#[tokio::test]
async fn stale_pending_melt_observation_after_failed_rollback_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let failed_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &failed_observation).await;

    let quote_after_failed = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_failed.state, MeltQuoteState::Unpaid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "failed rollback should clear the pending melt saga"
    );

    let stale_pending = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Pending,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &stale_pending).await;

    let quote_after_stale = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_stale.state, MeltQuoteState::Unpaid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after stale pending"),
        vec![None, None]
    );
    let observations = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("observation rows");
    assert_eq!(observations.len(), 2);
}

#[tokio::test]
async fn real_policy_failed_melt_observations_rollback_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(
        2,
        FederationPaymentTrustModel::SharedBackendReadOnly,
    );
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let first_observation = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 1,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let quote_after_single = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_single.state, MeltQuoteState::Pending);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "single failed observation must not compensate pending inputs"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after single failed observation"),
        vec![Some(State::Pending), Some(State::Pending)]
    );

    let matching_second = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 2,
                state: MeltQuoteState::Failed,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: None,
                failure_reason: Some("route failed"),
            },
        ),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let quote_after_quorum = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_quorum.state, MeltQuoteState::Unpaid);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_none(),
        "matching failed quorum should clear the pending melt saga"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after rollback"),
        vec![None, None]
    );

    let conflicting_paid = appended_entry(
        &mut journal,
        melt_payment_observation_envelope(
            &runtime_config,
            &quote,
            MeltPaymentObservationFixture {
                observer: 3,
                state: MeltQuoteState::Paid,
                amount: Amount::from(1),
                fee_reserve: Amount::from(1),
                payment_id: None,
                payment_proof: Some("late-preimage"),
                failure_reason: None,
            },
        ),
    );
    persist_and_apply_entry(&mint, &conflicting_paid).await;

    let quote_after_conflict = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("melt quote");
    assert_eq!(quote_after_conflict.state, MeltQuoteState::Unpaid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after conflict"),
        vec![None, None]
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting melt observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
}
