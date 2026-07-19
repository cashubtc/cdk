use super::*;

async fn persist_paid_mint_quote_projection_without_quote_journal(
    localstore: &Arc<MintSqliteDatabase>,
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    amount: u64,
    observation_entry: &FederationJournalEntry,
) {
    let quote_envelope = runtime_config.mint_quote_operation(
        quote.clone(),
        bolt11_quote_request(amount),
        bolt11_quote_response(quote, amount),
    );
    let FederationOperation::MintQuote(quote_operation) = &quote_envelope.operation else {
        panic!("expected mint quote operation");
    };
    let materialized_quote = super::super::mint_quote_from_federation_operation(quote_operation)
        .expect("materialized mint quote");

    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_mint_quote(materialized_quote)
        .await
        .expect("add materialized mint quote");
    tx.commit()
        .await
        .expect("commit materialized mint quote projection");

    persist_paid_mint_payment_projection(localstore, quote, amount, observation_entry).await;
}

async fn persist_paid_mint_payment_projection(
    localstore: &Arc<MintSqliteDatabase>,
    quote: &QuoteId,
    amount: u64,
    observation_entry: &FederationJournalEntry,
) {
    let observation_record =
        cdk_common::federation::FederationPaymentObservationRecord::from_journal_entry(
            observation_entry,
        )
        .expect("mint payment observation record");

    let mut tx = localstore.begin_transaction().await.expect("begin");
    let mut stored_quote = tx
        .get_mint_quote(quote)
        .await
        .expect("materialized mint quote lookup")
        .expect("materialized mint quote");
    stored_quote
        .add_payment(
            Amount::new(amount, CurrencyUnit::Sat),
            format!("materialized-payment-{quote}"),
            Some(0),
        )
        .expect("mark materialized mint quote paid");
    tx.update_mint_quote(&mut stored_quote)
        .await
        .expect("persist materialized mint quote payment");
    tx.add_federation_payment_observation(&observation_record)
        .await
        .expect("persist materialized payment observation");
    tx.commit()
        .await
        .expect("commit materialized payment projection");
}

#[tokio::test]
async fn apply_federation_mint_uses_rotated_keyset_for_signing() {
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

    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("rotated-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let effective = mint
        .effective_federation_config()
        .await
        .expect("effective config")
        .expect("federation config");
    let output = output_for_keyset(&next_keyset, b"rotated mint output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        effective.mint_operation(input).expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored signatures");
    let signature = signatures
        .first()
        .and_then(Option::as_ref)
        .expect("rotated blind signature");
    assert_eq!(signature.keyset_id, next_keyset.id);
    assert_eq!(signature.amount, Amount::from(1));

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("stored federation signature shares");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, effective.member_id());
    assert_eq!(share_records[0].keyset_id, next_keyset.id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );

    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("stored federation signature shares by blinded message");
    assert_eq!(blinded_message_share_records, share_records);
}

#[tokio::test]
async fn restore_returns_persisted_accepted_mint_shares_by_original_output_index() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let keyset = runtime_config.public_config().keysets[0].clone();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(3),
            bolt11_quote_response(&quote, 3),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(3),
            Amount::ZERO,
            Some("restore-persisted-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let outputs = vec![
        output_for_keyset(&keyset, b"restore accepted mint output zero"),
        output_for_keyset(&keyset, b"restore accepted mint output one"),
        output_for_keyset(&keyset, b"restore accepted mint output two"),
    ];
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: outputs.clone(),
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let restore_outputs = vec![outputs[2].clone(), outputs[0].clone()];
    let restored = mint
        .restore(RestoreRequest {
            outputs: restore_outputs.clone(),
        })
        .await
        .expect("restore accepted mint outputs");
    assert_eq!(restored.outputs, restore_outputs);
    assert_eq!(restored.signatures.len(), restored.outputs.len());

    for (restored_position, original_index) in [2_u32, 0_u32].into_iter().enumerate() {
        let restored_output = &restored.outputs[restored_position];
        let restored_signature = &restored.signatures[restored_position];
        let share_records = localstore
            .get_federation_signature_shares_for_output(mint_entry.operation_id, original_index)
            .await
            .expect("stored federation signature share by original output index");
        assert_eq!(share_records.len(), 1);
        let share_record = &share_records[0];
        assert_eq!(share_record.operation_id, mint_entry.operation_id);
        assert_eq!(share_record.output_index, original_index);
        assert_eq!(share_record.member_id, runtime_config.member_id());
        assert_eq!(share_record.keyset_id, keyset.id);
        assert_eq!(share_record.amount, Amount::from(1));
        assert_eq!(share_record.blinded_secret, restored_output.blinded_secret);
        assert_eq!(
            share_record.accepted_operation_index,
            Some(mint_entry.index)
        );
        assert_eq!(
            share_record.consensus_evidence,
            mint_entry.consensus_evidence
        );
        assert_eq!(restored_signature.keyset_id, keyset.id);
        assert_eq!(restored_signature.amount, Amount::from(1));
        assert_eq!(
            restored_signature.c,
            PublicKey::from(share_record.share.signature())
        );

        let blinded_message_share_records = localstore
            .get_federation_signature_shares_for_blinded_message(restored_output.blinded_secret)
            .await
            .expect("stored federation signature share by blinded message");
        assert_eq!(blinded_message_share_records, share_records);
    }
}

#[tokio::test]
async fn restore_ignores_orphan_blind_signature_without_federation_share() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let keyset = runtime_config.public_config().keysets[0].clone();
    let output = output_for_keyset(&keyset, b"orphan federation restore output");
    let orphan_signature = BlindSignature {
        amount: output.amount,
        keyset_id: output.keyset_id,
        c: PublicKey::from(BlsG1PublicKey::hash_to_curve(
            b"orphan federation restore signature",
        )),
        dleq: None,
    };

    let mut tx = localstore
        .begin_transaction()
        .await
        .expect("orphan signature transaction");
    tx.add_blind_signatures(
        &[output.blinded_secret],
        std::slice::from_ref(&orphan_signature),
        None,
    )
    .await
    .expect("store orphan compatibility signature");
    tx.commit()
        .await
        .expect("commit orphan compatibility signature");

    let compatibility_signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("orphan compatibility signature lookup");
    assert_eq!(compatibility_signatures, vec![Some(orphan_signature)]);
    assert!(
        localstore
            .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
            .await
            .expect("orphan federation share lookup")
            .is_empty(),
        "test fixture must contain no accepted-operation federation share"
    );

    let restored = mint
        .restore(RestoreRequest {
            outputs: vec![output],
        })
        .await
        .expect("restore orphan output");
    assert!(
        restored.outputs.is_empty(),
        "federation restore must not expose outputs backed only by compatibility signatures"
    );
    assert!(
        restored.signatures.is_empty(),
        "federation restore must not return signatures without accepted share provenance"
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_unpaid_quote_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let output = output_for_runtime(&runtime_config, b"unpaid federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("unpaid mint signature lookup");
    assert_eq!(signatures.len(), 1);
    assert!(
        signatures.iter().all(Option::is_none),
        "unpaid quote must fail before blind signature storage"
    );

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("unpaid mint share lookup");
    assert!(
        share_records.is_empty(),
        "unpaid quote must fail before federation signature share storage"
    );

    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("unpaid blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "unpaid quote must fail before blinded-message share storage"
    );

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Unpaid);

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_mint_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("stored mint entry");
    assert_eq!(
        stored_mint_entry.status(),
        FederationOperationStatus::Accepted
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_locally_paid_quote_without_consensus_observation() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let mut tx = localstore.begin_transaction().await.expect("begin");
    let mut stored_quote = tx
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    stored_quote
        .add_payment(
            Amount::new(1, CurrencyUnit::Sat),
            "local-only-payment".to_string(),
            None,
        )
        .expect("mark quote locally paid");
    tx.update_mint_quote(&mut stored_quote)
        .await
        .expect("persist local payment");
    tx.commit().await.expect("commit local payment");

    let output = output_for_runtime(&runtime_config, b"local-only paid mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote,
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup"),
        vec![None]
    );
    assert!(localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("share lookup")
        .is_empty());
}

#[tokio::test]
async fn apply_federation_mint_rejects_materialized_quote_without_prior_quote_operation() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let observation_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("materialized-without-quote-operation".to_string()),
        ),
    );
    persist_paid_mint_quote_projection_without_quote_journal(
        &localstore,
        &runtime_config,
        &quote,
        1,
        &observation_entry,
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"mint without quote provenance");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnknownQuote)
    ));
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup"),
        vec![None]
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_quote_applied_only_at_later_index() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let observation_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("materialized-before-late-quote".to_string()),
        ),
    );
    persist_paid_mint_quote_projection_without_quote_journal(
        &localstore,
        &runtime_config,
        &quote,
        1,
        &observation_entry,
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"mint before quote operation");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    assert!(quote_entry.index > mint_entry.index);
    persist_and_apply_entry(&mint, &quote_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnknownQuote)
    ));
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup"),
        vec![None]
    );
}

#[tokio::test]
async fn mint_payment_observation_at_same_index_does_not_authorize_mint() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let observation_entry = accepted_entry_without_journal(
        1,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("same-index-payment".to_string()),
        ),
    );
    persist_paid_mint_payment_projection(&localstore, &quote, 1, &observation_entry).await;

    let output = output_for_runtime(&runtime_config, b"same index payment observation");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    assert_eq!(observation_entry.index, mint_entry.index);
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup"),
        vec![None]
    );
}

#[tokio::test]
async fn later_mint_payment_quorum_does_not_retroactively_authorize_older_mint() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let output = output_for_runtime(&runtime_config, b"pre-payment-quorum mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;
    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));

    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("later-payment".to_string()),
        ),
    );
    assert!(paid_entry.index > mint_entry.index);
    persist_and_apply_entry(&mint, &paid_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnpaidQuote)
    ));
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup"),
        vec![None]
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_duplicate_outputs_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&quote, 2),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(2),
            Amount::ZERO,
            Some("duplicate-output-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"duplicate federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::Mint(FederationMintOperation {
            request: FederationMintRequest::Single(MintRequest {
                quote: quote.to_string(),
                outputs: vec![output.clone(), output.clone()],
                signature: None,
            }),
        })),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::DuplicateOutputs)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("duplicate-output mint signature lookup");
    assert_eq!(
        signatures,
        vec![None],
        "duplicate outputs must fail before blind signature storage"
    );
    for output_index in [0, 1] {
        let share_records = localstore
            .get_federation_signature_shares_for_output(mint_entry.operation_id, output_index)
            .await
            .expect("duplicate-output mint share lookup");
        assert!(
            share_records.is_empty(),
            "duplicate outputs must fail before federation share storage"
        );
    }
    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("duplicate-output blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "duplicate outputs must fail before blinded-message share storage"
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_completed_operations()
            .await
            .expect("completed operation lookup")
            .is_empty(),
        "duplicate outputs must fail before completed-operation storage"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_quote_amount_mismatch_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(2),
            bolt11_quote_response(&quote, 2),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(2),
            Amount::ZERO,
            Some("amount-mismatch-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"amount mismatch federation mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(BatchMintRequest {
                quotes: vec![quote.clone()],
                quote_amounts: Some(vec![Amount::from(2)]),
                outputs: vec![output.clone()],
                signatures: None,
            }))
            .expect("amount-mismatch batch mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::TransactionUnbalanced(2, 1, 0))
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("amount-mismatch mint signature lookup");
    assert_eq!(
        signatures,
        vec![None],
        "amount mismatch must fail before blind signature storage"
    );
    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("amount-mismatch mint share lookup");
    assert!(
        share_records.is_empty(),
        "amount mismatch must fail before federation share storage"
    );
    let blinded_message_share_records = localstore
        .get_federation_signature_shares_for_blinded_message(output.blinded_secret)
        .await
        .expect("amount-mismatch blinded-message share lookup");
    assert!(
        blinded_message_share_records.is_empty(),
        "amount mismatch must fail before blinded-message share storage"
    );
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert!(
        localstore
            .get_completed_operations()
            .await
            .expect("completed operation lookup")
            .is_empty(),
        "amount mismatch must fail before completed-operation storage"
    );
}

#[tokio::test]
async fn apply_federation_mint_persists_accepted_materialization_rows() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("accepted-materialization-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"accepted materialized mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("accepted materialization mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Issued);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::new(1, CurrencyUnit::Sat)
    );

    let operation_uuid = super::super::federation_operation_uuid(mint_entry.operation_id);
    let completed_operation = localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup")
        .expect("completed operation");
    assert_eq!(
        completed_operation.kind(),
        cdk_common::mint::OperationKind::Mint
    );
    assert_eq!(completed_operation.total_issued(), Amount::from(1));
    assert_eq!(completed_operation.total_redeemed(), Amount::ZERO);
    assert_eq!(completed_operation.fee_collected(), Amount::ZERO);
    assert_eq!(completed_operation.completed_at(), &Some(0));
    assert_eq!(
        completed_operation.payment_method(),
        Some(PaymentMethod::Known(KnownMethod::Bolt11))
    );

    let blinded_secrets = localstore
        .get_blinded_secrets_by_operation_id(&operation_uuid)
        .await
        .expect("blinded secrets by operation id");
    assert_eq!(blinded_secrets, vec![output.blinded_secret]);

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored blind signature");
    let signature = signatures[0].as_ref().expect("accepted mint signature");
    assert_eq!(signature.amount, Amount::from(1));
    assert_eq!(signature.keyset_id, output.keyset_id);

    let share_records = localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("stored federation signature share");
    assert_eq!(share_records.len(), 1);
    assert_eq!(share_records[0].operation_id, mint_entry.operation_id);
    assert_eq!(share_records[0].output_index, 0);
    assert_eq!(share_records[0].member_id, runtime_config.member_id());
    assert_eq!(share_records[0].keyset_id, output.keyset_id);
    assert_eq!(share_records[0].amount, Amount::from(1));
    assert_eq!(share_records[0].blinded_secret, output.blinded_secret);
    assert_eq!(
        share_records[0].accepted_operation_index,
        Some(mint_entry.index)
    );
    assert_eq!(
        share_records[0].consensus_evidence,
        mint_entry.consensus_evidence
    );

    let stored_entries = localstore
        .get_federation_journal_entries()
        .await
        .expect("journal lookup");
    let stored_mint_entry = stored_entries
        .iter()
        .find(|entry| entry.operation_id == mint_entry.operation_id)
        .expect("stored mint entry");
    assert_eq!(
        stored_mint_entry.status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn catch_up_replay_applies_ordered_mint_history_and_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    let observation_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("catch-up-mint-payment".to_string()),
        ),
    );
    let output = output_for_runtime(&runtime_config, b"catch-up mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("mint operation"),
    );
    let entries = vec![quote_entry, observation_entry, mint_entry];

    let mut tx = localstore.begin_transaction().await.expect("begin");
    for entry in &entries {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist accepted catch-up entry");
    }
    tx.commit().await.expect("commit accepted catch-up entries");

    let applied = mint
        .apply_federation_journal_entries(&entries)
        .await
        .expect("apply ordered catch-up history");
    assert_eq!(
        applied,
        entries
            .iter()
            .map(|entry| entry.operation_id)
            .collect::<Vec<_>>()
    );
    assert!(localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("catch-up signature lookup")[0]
        .is_some());
    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("catch-up quote lookup")
        .expect("catch-up quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Issued);
    assert!(localstore
        .get_federation_journal_entries()
        .await
        .expect("catch-up journal lookup")
        .iter()
        .all(|entry| entry.status() == FederationOperationStatus::Applied));

    assert!(mint
        .apply_federation_journal_entries(&entries)
        .await
        .expect("idempotent restart replay")
        .is_empty());
}

#[tokio::test]
async fn apply_federation_mint_rolls_back_late_failure_and_replays() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let paid_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("late-failure-mint-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &paid_entry).await;

    let output = output_for_runtime(&runtime_config, b"late failure mint output");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(MintRequest {
                quote: quote.clone(),
                outputs: vec![output.clone()],
                signature: None,
            }))
            .expect("late failure mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    crate::test_helpers::mint::set_fail_for("FEDERATION_MINT_APPLY_BEFORE_STATUS");
    let result = mint
        .apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
        .await;
    crate::test_helpers::mint::clear_fail_for("FEDERATION_MINT_APPLY_BEFORE_STATUS");
    assert!(matches!(result, Err(Error::Internal)));

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup after rollback")
        .expect("quote after rollback");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(
        stored_quote.amount_issued(),
        Amount::ZERO.with_unit(CurrencyUnit::Sat)
    );
    assert_eq!(
        localstore
            .get_blind_signatures(&[output.blinded_secret])
            .await
            .expect("signature lookup after rollback"),
        vec![None]
    );
    assert!(localstore
        .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
        .await
        .expect("share lookup after rollback")
        .is_empty());
    let operation_uuid = super::super::federation_operation_uuid(mint_entry.operation_id);
    assert!(localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed operation lookup after rollback")
        .is_none());
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
            .find(|entry| entry.operation_id == mint_entry.operation_id)
            .expect("mint entry after rollback")
            .status(),
        FederationOperationStatus::Accepted
    );

    assert_eq!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await
            .expect("replay mint after rollback"),
        vec![mint_entry.operation_id]
    );
    let replayed_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup after replay")
        .expect("quote after replay");
    assert_eq!(replayed_quote.state(), MintQuoteState::Issued);
    assert_eq!(
        replayed_quote.amount_issued(),
        Amount::new(1, CurrencyUnit::Sat)
    );
    assert!(localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("signature lookup after replay")[0]
        .is_some());
    assert_eq!(
        localstore
            .get_federation_signature_shares_for_output(mint_entry.operation_id, 0)
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
async fn apply_federation_mint_accepts_nut20_locked_quote() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-valid-payment",
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"nut20 valid mint output");
    let mut request = MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    };
    request.sign(&signing_key).expect("nut20 signature");
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(request))
            .expect("locked mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("stored locked mint signature");
    assert!(
        signatures[0].is_some(),
        "valid NUT-20 locked quote must receive a member share"
    );
    let quote_state = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_state.state(), MintQuoteState::Issued);
    assert_eq!(
        quote_state.amount_issued(),
        Amount::new(1, CurrencyUnit::Sat)
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_missing_nut20_signature_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-missing-payment",
    )
    .await;

    let output = output_for_runtime(&runtime_config, b"nut20 missing signature output");
    let input = MintInput::Single(MintRequest {
        quote: quote.clone(),
        outputs: vec![output.clone()],
        signature: None,
    });
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(input)
            .expect("missing signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));

    let signatures = localstore
        .get_blind_signatures(&[output.blinded_secret])
        .await
        .expect("missing signature output lookup");
    assert_eq!(
        signatures,
        vec![None],
        "missing NUT-20 signature must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_mint_rejects_nut20_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let signing_key = SecretKey::generate();
    let quote = QuoteId::new();
    persist_paid_locked_mint_quote(
        &mint,
        &mut journal,
        &runtime_config,
        &quote,
        signing_key.public_key(),
        "nut20-mutated-payment",
    )
    .await;

    let signed_output = output_for_runtime(&runtime_config, b"nut20 signed mint output");
    let mutated_output = output_for_runtime(&runtime_config, b"nut20 mutated mint output");
    let mut request = MintRequest {
        quote: quote.clone(),
        outputs: vec![signed_output.clone()],
        signature: None,
    };
    request.sign(&signing_key).expect("nut20 signature");
    request.outputs = vec![mutated_output.clone()];
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Single(request))
            .expect("mutated locked mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));

    let signatures = localstore
        .get_blind_signatures(&[signed_output.blinded_secret, mutated_output.blinded_secret])
        .await
        .expect("mutated signature lookup");
    assert_eq!(
        signatures,
        vec![None, None],
        "mutated NUT-20 output set must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_any_quote_without_prior_quote_operation() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let valid_quote = QuoteId::new();
    let valid_quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            valid_quote.clone(),
            bolt11_quote_request(1),
            bolt11_quote_response(&valid_quote, 1),
        ),
    );
    persist_and_apply_entry(&mint, &valid_quote_entry).await;
    let valid_observation_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            valid_quote.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("valid-batch-payment".to_string()),
        ),
    );
    persist_and_apply_entry(&mint, &valid_observation_entry).await;

    let quote_without_provenance = QuoteId::new();
    let projected_observation_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_payment_observation(
            quote_without_provenance.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintQuoteState::Paid,
            Amount::from(1),
            Amount::ZERO,
            Some("projected-batch-payment".to_string()),
        ),
    );
    persist_paid_mint_quote_projection_without_quote_journal(
        &localstore,
        &runtime_config,
        &quote_without_provenance,
        1,
        &projected_observation_entry,
    )
    .await;

    let outputs = vec![
        output_for_runtime(&runtime_config, b"valid provenance batch output"),
        output_for_runtime(&runtime_config, b"missing provenance batch output"),
    ];
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(BatchMintRequest {
                quotes: vec![valid_quote.clone(), quote_without_provenance.clone()],
                quote_amounts: None,
                outputs: outputs.clone(),
                signatures: None,
            }))
            .expect("batch mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(Error::UnknownQuote)
    ));
    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    assert_eq!(
        localstore
            .get_blind_signatures(&blinded_secrets)
            .await
            .expect("batch signature lookup"),
        vec![None, None]
    );
    for quote in [&valid_quote, &quote_without_provenance] {
        let stored_quote = localstore
            .get_mint_quote(quote)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(stored_quote.state(), MintQuoteState::Paid);
        assert_eq!(
            stored_quote.amount_issued(),
            Amount::ZERO.with_unit(CurrencyUnit::Sat)
        );
    }
}

#[tokio::test]
async fn apply_federation_batch_mint_accepts_nut20_locked_quotes() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-second-payment",
    )
    .await;

    let outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 first output"),
        output_for_runtime(&runtime_config, b"batch nut20 second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch locked mint operation"),
    );
    persist_and_apply_entry(&mint, &mint_entry).await;

    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("stored batch locked mint signatures");
    assert_eq!(signatures.len(), 2);
    assert!(
        signatures.iter().all(Option::is_some),
        "valid batch NUT-20 locked quotes must receive member shares"
    );
    for quote in [&first_quote, &second_quote] {
        let quote_state = localstore
            .get_mint_quote(quote)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(quote_state.state(), MintQuoteState::Issued);
        assert_eq!(
            quote_state.amount_issued(),
            Amount::new(1, CurrencyUnit::Sat)
        );
    }
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_missing_nut20_signature_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-missing-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-missing-second-payment",
    )
    .await;

    let outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 missing first output"),
        output_for_runtime(&runtime_config, b"batch nut20 missing second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    if let Some(signatures) = request.signatures.as_mut() {
        signatures[1] = None;
    }
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch missing signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));
    let blinded_secrets = outputs
        .iter()
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("missing batch signature lookup");
    assert_eq!(
        signatures,
        vec![None, None],
        "batch missing NUT-20 signature must fail before any member share is stored"
    );
}

#[tokio::test]
async fn apply_federation_batch_mint_rejects_nut20_output_mutation_before_signing() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let first_key = SecretKey::generate();
    let second_key = SecretKey::generate();
    let first_quote = QuoteId::new();
    let second_quote = QuoteId::new();
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &first_quote,
        1,
        first_key.public_key(),
        "batch-nut20-mutated-first-payment",
    )
    .await;
    persist_paid_locked_mint_quote_amount(
        &mint,
        &mut journal,
        &runtime_config,
        &second_quote,
        1,
        second_key.public_key(),
        "batch-nut20-mutated-second-payment",
    )
    .await;

    let signed_outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 signed first output"),
        output_for_runtime(&runtime_config, b"batch nut20 signed second output"),
    ];
    let mutated_outputs = vec![
        output_for_runtime(&runtime_config, b"batch nut20 mutated first output"),
        output_for_runtime(&runtime_config, b"batch nut20 mutated second output"),
    ];
    let mut request = BatchMintRequest {
        quotes: vec![first_quote.clone(), second_quote.clone()],
        quote_amounts: None,
        outputs: signed_outputs.clone(),
        signatures: None,
    };
    sign_batch_mint_request_for_quotes(&mut request, &[first_key, second_key]);
    request.outputs = mutated_outputs.clone();
    let mint_entry = appended_entry(
        &mut journal,
        runtime_config
            .mint_operation(MintInput::Batch(request))
            .expect("batch mutated signature mint operation"),
    );
    persist_journal_entry(&localstore, &mint_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&mint_entry))
            .await,
        Err(cdk_common::Error::SignatureMissingOrInvalid)
    ));
    let blinded_secrets = signed_outputs
        .iter()
        .chain(mutated_outputs.iter())
        .map(|output| output.blinded_secret)
        .collect::<Vec<_>>();
    let signatures = localstore
        .get_blind_signatures(&blinded_secrets)
        .await
        .expect("mutated batch signature lookup");
    assert_eq!(
        signatures,
        vec![None, None, None, None],
        "batch mutated NUT-20 output set must fail before any member share is stored"
    );
}
