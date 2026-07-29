use super::*;

#[tokio::test]
async fn accepted_frost_onchain_melt_materializes_one_pending_payment_intent() {
    use cdk_common::bitcoin::{hashes::Hash, BlockHash, OutPoint, Txid};
    use cdk_common::bitcoin::{Address, Network};
    use cdk_common::federation::FederationOnchainMeltPaymentReceipt;
    use cdk_common::nuts::nut30::{
        MeltQuoteOnchainFeeOption, MeltQuoteOnchainRequest, MeltQuoteOnchainResponse,
    };

    let (runtime_config, _) = super::address_allocation::runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let destination_script = runtime_config
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("FROST wallet")
        .derive_cdk_onchain_script(
            runtime_config.public_config(),
            runtime_config
                .cdk_onchain_application()
                .expect("application")
                .application_id,
            3,
            cdk_common::federation::FederationAddressKeychain::External,
            90,
        )
        .expect("destination script");
    let destination = Address::from_script(&destination_script, Network::Regtest)
        .expect("regtest destination")
        .to_string();
    let quote = QuoteId::new();
    let request = MeltQuoteRequest::Onchain(MeltQuoteOnchainRequest {
        request: destination.clone(),
        unit: CurrencyUnit::Sat,
        amount: Amount::from(1),
    });
    let fee_option = MeltQuoteOnchainFeeOption {
        fee_index: 7,
        fee_reserve: Amount::from(1),
        estimated_blocks: 6,
    };
    let response = MeltQuoteCreateResponse::Onchain(MeltQuoteOnchainResponse {
        quote: quote.clone(),
        amount: Amount::from(1),
        unit: CurrencyUnit::Sat,
        method: PaymentMethod::Known(KnownMethod::Onchain),
        state: MeltQuoteState::Unpaid,
        expiry: 4_000_000_000,
        request: destination,
        fee_options: vec![fee_option],
        selected_fee_index: None,
        outpoint: None,
        change: None,
    });
    let metadata = FederationMeltQuoteMetadata {
        created_time: 1_700_000_000,
        request_lookup_id: Some(PaymentIdentifier::QuoteId(quote.clone())),
        extra_json: None,
        estimated_blocks: Some(6),
    };
    let mut journal = InMemoryFederationJournal::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(quote.clone(), request, response, metadata),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "frost-onchain-input-one"),
        proof_for_runtime(&runtime_config, "frost-onchain-input-two"),
    ];
    let input_ys = proofs.ys().expect("on-chain melt input ys");
    let melt = MeltRequest::new(quote.clone(), proofs, None).fee_index(7);
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(melt)
            .expect("on-chain melt operation"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    let stored_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("stored quote");
    assert_eq!(stored_quote.state, MeltQuoteState::Pending);
    let state = mint
        .federation_transaction_state()
        .await
        .expect("transaction state");
    let intent = state
        .intent_for_quote(&quote.to_string())
        .expect("pending payment intent");
    assert_eq!(intent.intent.amount_sat, 1);
    assert_eq!(intent.intent.maximum_fee_sat, 1);
    assert_eq!(intent.intent.fee_tier, 7);
    assert_eq!(
        intent.state,
        cdk_common::federation::FederationOnchainPaymentIntentState::Pending
    );
    assert_eq!(
        intent.intent.creation_time,
        super::super::federation_accepted_order(&melt_entry)
    );
    assert_eq!(intent.intent.melt_operation_id, melt_entry.operation_id);
    assert!(mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("joint-wallet payment probe")
        .is_none());
    let premature_txid = Txid::from_byte_array([32; 32]);
    let premature_observation = FederationMeltQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Onchain),
        state: MeltQuoteState::Paid,
        amount: Amount::from(1),
        fee_reserve: Amount::from(1),
        total_spent: Some(Amount::from(1)),
        payment_id: Some(PaymentIdentifier::QuoteId(quote.clone())),
        payment_proof: Some(OutPoint::new(premature_txid, 0).to_string()),
    };
    assert!(matches!(
        mint.validate_federation_onchain_melt_payment_observation(
            &stored_quote,
            &premature_observation
        )
        .await,
        Err(Error::Federation(
            cdk_common::federation::FederationError::OperationStateInvalid { .. }
        ))
    ));

    let (mut completed_state, signed) =
        super::transaction_signing::signed_fixture_for_intent(intent.intent.clone(), 1);
    completed_state
        .mark_transaction_signed(signed.proposal_id, signed.clone())
        .expect("persist signed transaction");
    let settlement_policy = runtime_config
        .onchain_transaction_settlement_policy(intent.intent.scope)
        .expect("settlement policy");
    completed_state
        .authorize_transaction_broadcast(
            cdk_common::federation::FederationTransactionBroadcastIntent {
                proposal_id: signed.proposal_id,
                txid: signed.txid,
                settlement_policy,
            },
            FederationOperationId::from_bytes([35; 32]),
            11,
        )
        .expect("authorize broadcast");
    for (offset, observer) in runtime_config
        .public_config()
        .members
        .keys()
        .copied()
        .take(usize::from(settlement_policy.required_observers))
        .enumerate()
    {
        completed_state
            .record_transaction_broadcast_observation(
                cdk_common::federation::FederationTransactionBroadcastObservation {
                    proposal_id: signed.proposal_id,
                    txid: signed.txid,
                    observer,
                    attempt: 1,
                    result:
                        cdk_common::federation::FederationTransactionBroadcastResult::Confirmed {
                            block_hash: BlockHash::from_byte_array([36; 32]),
                            block_height: 200,
                            confirmation_depth: settlement_policy.minimum_confirmations,
                        },
                    detail: None,
                },
                FederationOperationId::from_bytes(
                    [u8::try_from(offset + 37).expect("operation marker"); 32],
                ),
                u64::try_from(offset + 12).expect("operation index"),
            )
            .expect("record threshold confirmation");
    }
    super::transaction_signing::persist_transaction_state(&localstore, &completed_state).await;
    let terminal_observation = mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("completed joint-wallet payment probe")
        .expect("paid observation");
    let completed_receipt = completed_state
        .completed_payment_for_quote(&quote.to_string())
        .expect("completed receipt")
        .expect("receipt");
    assert_eq!(
        terminal_observation.payment_id,
        Some(PaymentIdentifier::QuoteId(quote.clone()))
    );
    assert_eq!(
        terminal_observation.payment_proof,
        Some(completed_receipt.outpoint.to_string())
    );
    let mut last_payment_entry = None;
    for observer in runtime_config
        .public_config()
        .members
        .keys()
        .copied()
        .take(usize::from(settlement_policy.required_observers))
    {
        let mut observation = terminal_observation.clone();
        observation.observer = observer;
        let entry = appended_entry(
            &mut journal,
            runtime_config.operation_envelope(FederationOperation::MeltQuotePayment(observation)),
        );
        persist_and_apply_entry(&mint, &entry).await;
        last_payment_entry = Some(entry);
    }
    let paid_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("paid quote lookup")
        .expect("paid quote");
    assert_eq!(paid_quote.state, MeltQuoteState::Paid);
    assert_eq!(
        paid_quote.payment_proof,
        Some(completed_receipt.outpoint.to_string())
    );
    assert!(localstore
        .get_proofs_states(&input_ys)
        .await
        .expect("paid input states")
        .into_iter()
        .all(|state| state == Some(State::Spent)));
    assert!(!mint
        .apply_accepted_federation_operation(
            last_payment_entry
                .as_ref()
                .expect("last payment observation")
        )
        .await
        .expect("payment observation replay"));

    let txid = Txid::from_byte_array([33; 32]);
    let receipt = FederationOnchainMeltPaymentReceipt {
        quote: quote.to_string(),
        melt_operation_id: melt_entry.operation_id,
        txid,
        outpoint: OutPoint::new(txid, 0),
        amount_sat: 1,
        fee_sat: 0,
    };
    let observation = FederationMeltQuotePaymentObservation {
        quote: quote.to_string(),
        observer: runtime_config.member_id(),
        method: PaymentMethod::Known(KnownMethod::Onchain),
        state: MeltQuoteState::Paid,
        amount: Amount::from(1),
        fee_reserve: Amount::from(1),
        total_spent: Some(Amount::from(1)),
        payment_id: Some(PaymentIdentifier::QuoteId(quote.clone())),
        payment_proof: Some(receipt.outpoint.to_string()),
    };
    let response = super::super::melt_quote_payment_response(
        FederationOperationId::from_bytes([34; 32]),
        &stored_quote,
        &observation,
        Some(&receipt),
    )
    .expect("existing on-chain response shape");
    assert_eq!(
        response.payment_lookup_id,
        PaymentIdentifier::QuoteId(quote)
    );
    assert_eq!(response.payment_proof, Some(receipt.outpoint.to_string()));
    assert_eq!(response.status, MeltQuoteState::Paid);
    assert_eq!(response.total_spent, Amount::new(1, CurrencyUnit::Sat));

    let exact_offchain_response = super::super::melt_quote_payment_response(
        FederationOperationId::from_bytes([35; 32]),
        &stored_quote,
        &observation,
        None,
    )
    .expect("exact off-chain response");
    assert_eq!(
        exact_offchain_response.total_spent,
        Amount::new(1, CurrencyUnit::Sat),
        "unused fee reserve must remain available as melt change"
    );
}

#[tokio::test]
async fn concurrent_same_federation_melt_quote_apply_is_idempotent() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_journal_entry(&localstore, &quote_entry).await;

    let (first, second) = tokio::join!(
        mint.apply_accepted_federation_operation(&quote_entry),
        mint.apply_accepted_federation_operation(&quote_entry),
    );

    let first = first.expect("first concurrent apply");
    let second = second.expect("second concurrent apply");
    assert!(
        first || second,
        "one concurrent apply must materialize the quote"
    );
    let persisted_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("melt quote lookup")
        .expect("persisted melt quote");
    assert_eq!(persisted_quote.id, quote);
    assert_eq!(persisted_quote.state, MeltQuoteState::Unpaid);
}

#[tokio::test]
async fn accepted_melt_quote_materializes_canonical_terms_without_follower_backend_call() {
    let runtime_config = runtime_config();
    let (proposer, proposer_store) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let request = pending_melt_quote_request(1_000);
    let (response, metadata) = proposer
        .prepare_melt_quote(request.clone())
        .await
        .expect("prepare canonical melt quote");
    let quote = response.quote().expect("melt quote id").clone();
    assert_eq!(response.quote(), Some(&quote));
    assert!(proposer_store
        .get_melt_quote(&quote)
        .await
        .expect("proposer quote lookup")
        .is_none());

    let quote_calls = Arc::new(AtomicUsize::new(0));
    let follower_backend = Arc::new(QuoteCountingBackendAgnosticPaymentProcessor {
        quote_calls: Arc::clone(&quote_calls),
    });
    let (follower, follower_store) =
        federated_test_mint_with_payment_processor(runtime_config.clone(), follower_backend).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(
            quote.clone(),
            request,
            response.clone(),
            metadata.clone(),
        ),
    );
    persist_and_apply_entry(&follower, &quote_entry).await;
    assert!(!follower
        .apply_accepted_federation_operation(&quote_entry)
        .await
        .expect("exact quote replay"));

    assert_eq!(quote_calls.load(Ordering::Relaxed), 0);
    let stored = follower_store
        .get_melt_quote(&quote)
        .await
        .expect("follower quote lookup")
        .expect("follower quote");
    assert_eq!(
        FederationMeltQuoteMetadata::from_melt_quote(&stored),
        metadata
    );
    let MeltQuoteCreateResponse::Bolt11(response) = response else {
        panic!("expected bolt11 response");
    };
    assert_eq!(
        stored.amount(),
        response.amount.with_unit(stored.unit.clone())
    );
    assert_eq!(
        stored.fee_reserve(),
        response.fee_reserve.with_unit(stored.unit.clone())
    );
    assert_eq!(stored.expiry, response.expiry);
    assert_eq!(stored.created_time, metadata.created_time);
}

#[tokio::test]
async fn accepted_melt_quote_rejects_conflicting_preexisting_quote_without_rewrite() {
    let runtime_config = runtime_config();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let quote = QuoteId::new();
    let request = pending_melt_quote_request(1_000);
    let response = test_melt_quote_response(&quote, &request);
    let envelope = runtime_config.melt_quote_operation(
        quote.clone(),
        request,
        response,
        FederationMeltQuoteMetadata::default(),
    );
    let FederationOperation::MeltQuote(operation) = &envelope.operation else {
        panic!("expected melt quote operation");
    };
    let mut conflicting = operation
        .into_melt_quote(quote.clone())
        .expect("conflicting local quote");
    conflicting.created_time = 999;
    let mut tx = localstore.begin_transaction().await.expect("begin");
    tx.add_melt_quote(conflicting)
        .await
        .expect("insert conflicting quote");
    tx.commit().await.expect("commit conflicting quote");

    let mut journal = InMemoryFederationJournal::new();
    let quote_entry = appended_entry(&mut journal, envelope);
    persist_journal_entry(&localstore, &quote_entry).await;
    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&quote_entry))
            .await,
        Err(Error::InvalidPaymentRequest)
    ));
    assert_eq!(
        localstore
            .get_melt_quote(&quote)
            .await
            .expect("conflicting quote lookup")
            .expect("conflicting quote")
            .created_time,
        999
    );
}

#[tokio::test]
async fn standalone_melt_quote_creation_still_persists_quote() {
    let (mint, localstore) = standalone_test_mint_with_backend_agnostic_payment().await;
    let request = pending_melt_quote_request(1_000);
    let response = mint
        .get_melt_quote(request)
        .await
        .expect("standalone melt quote");
    let quote = response.quote().cloned().expect("quote id");
    let stored = localstore
        .get_melt_quote(&quote)
        .await
        .expect("standalone quote lookup")
        .expect("standalone quote persisted");
    assert_eq!(stored.id, quote);
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
}

#[tokio::test]
async fn accepted_melt_author_is_the_only_payment_actor() {
    let mut configs = runtime_configs();
    let author_config = configs.remove(0);
    let follower_config = configs.remove(0);
    let (author, author_store, author_payment_calls) =
        federated_test_mint_with_counting_payment_processor(author_config.clone()).await;
    let (follower, follower_store, follower_payment_calls) =
        federated_test_mint_with_counting_payment_processor(follower_config).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &author_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&author, &quote_entry).await;
    persist_and_apply_entry(&follower, &quote_entry).await;

    let melt_entry = (0..64)
        .find_map(|attempt| {
            let proofs = vec![
                proof_for_runtime(
                    &author_config,
                    &format!("author-payment-input-{attempt}-one"),
                ),
                proof_for_runtime(
                    &author_config,
                    &format!("author-payment-input-{attempt}-two"),
                ),
            ];
            let envelope = author_config
                .melt_operation(MeltRequest::new(quote.clone(), proofs, None))
                .expect("melt operation");
            let operation_id = envelope.operation_id().expect("melt operation id");
            let selector = u64::from_be_bytes(
                operation_id.to_bytes()[..8]
                    .try_into()
                    .expect("operation selector bytes"),
            );
            (selector % 2 == 1).then(|| appended_entry(&mut journal, envelope))
        })
        .expect("operation whose old roster selector points at the follower");
    assert_eq!(
        melt_entry
            .envelope
            .verify_authorization(author_config.public_config())
            .expect("melt operation authorization"),
        author_config.member_id()
    );
    persist_and_apply_entry(&author, &melt_entry).await;
    persist_and_apply_entry(&follower, &melt_entry).await;

    let _ = follower.probe_federation_melt_quote_payment(&quote).await;
    assert_eq!(follower_payment_calls.load(Ordering::Relaxed), 0);
    assert!(author
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("author payment probe")
        .is_none());
    assert_eq!(author_payment_calls.load(Ordering::Relaxed), 1);
    assert!(author_store
        .get_federation_journal_entry_by_operation_id(melt_entry.operation_id)
        .await
        .expect("author journal lookup")
        .is_some());
    assert!(follower_store
        .get_federation_journal_entry_by_operation_id(melt_entry.operation_id)
        .await
        .expect("follower journal lookup")
        .is_some());
}

#[tokio::test]
async fn concurrent_same_quote_payment_probe_has_one_durable_winner() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls, first_started, release_first) =
        federated_test_mint_with_blocking_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, _) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;

    let first_mint = mint.clone();
    let first_quote = quote.clone();
    let first = tokio::spawn(async move {
        first_mint
            .probe_federation_melt_quote_payment(&first_quote)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), first_started.acquire())
        .await
        .expect("first payment attempt start timeout")
        .expect("first payment start semaphore closed")
        .forget();

    let duplicate = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mint.probe_federation_melt_quote_payment(&quote),
    )
    .await
    .expect("duplicate probe must not wait for the payment backend")
    .expect("duplicate probe");
    assert!(duplicate.is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);

    release_first.add_permits(1);
    assert!(first
        .await
        .expect("first probe task")
        .expect("first probe")
        .is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn unrelated_quote_payment_probes_do_not_serialize() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls, first_started, release_first) =
        federated_test_mint_with_blocking_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (first_quote, _) = setup_pending_federated_melt_with_label(
        &mint,
        &localstore,
        &runtime_config,
        &mut journal,
        "first-concurrent-melt",
    )
    .await;
    let (second_quote, _) = setup_pending_federated_melt_with_label(
        &mint,
        &localstore,
        &runtime_config,
        &mut journal,
        "second-concurrent-melt",
    )
    .await;

    let first_mint = mint.clone();
    let first = tokio::spawn(async move {
        first_mint
            .probe_federation_melt_quote_payment(&first_quote)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), first_started.acquire())
        .await
        .expect("first payment attempt start timeout")
        .expect("first payment start semaphore closed")
        .forget();

    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        mint.probe_federation_melt_quote_payment(&second_quote),
    )
    .await
    .expect("unrelated quote must proceed while first backend call is blocked")
    .expect("second probe");
    assert!(second.is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 2);

    release_first.add_permits(1);
    assert!(first
        .await
        .expect("first probe task")
        .expect("first probe")
        .is_none());
}

#[tokio::test]
async fn later_melt_quote_does_not_retroactively_authorize_older_melt() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let proofs = vec![
        proof_for_runtime(&runtime_config, "pre-quote-melt-input-one"),
        proof_for_runtime(&runtime_config, "pre-quote-melt-input-two"),
    ];
    let input_ys = proofs.ys().expect("pre-quote input ys");
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(quote.clone(), proofs, None))
            .expect("melt operation"),
    );
    persist_journal_entry(&localstore, &melt_entry).await;
    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::UnknownQuote)
    ));

    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    assert!(quote_entry.index > melt_entry.index);
    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await,
        Err(Error::UnknownQuote)
    ));
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("pre-quote input states"),
        vec![None, None]
    );
}

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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            secret_quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            witness_quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
    let previous_order_digest = FederationJournalDigest::from_bytes([1; 32]);
    let order_digest = FederationJournalDigest::from_bytes([2; 32]);
    let conflicting_entry = FederationJournalEntry {
        index: 42,
        operation_id: conflicting_operation_id,
        conflict_keys: conflicting_envelope.operation.conflict_keys(),
        envelope: conflicting_envelope,
        previous_order_digest,
        order_digest,
        consensus_evidence: None,
        application: FederationApplicationState::Accepted {},
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
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
            .status(),
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
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
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
            .status(),
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
async fn apply_federation_melt_rejects_distinct_quote_for_paid_lookup_id_before_payment() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let request = pending_melt_quote_request(1_000);
    let MeltQuoteRequest::Bolt11(bolt11_request) = &request else {
        panic!("expected bolt11 test request");
    };
    let request_lookup_id =
        PaymentIdentifier::PaymentHash(*bolt11_request.request.payment_hash().as_ref());
    let original_quote = QuoteId::new();
    let duplicate_quote = QuoteId::new();

    for quote in [&original_quote, &duplicate_quote] {
        let response = test_melt_quote_response(quote, &request);
        let entry = appended_entry(
            &mut journal,
            runtime_config.melt_quote_operation(
                quote.clone(),
                request.clone(),
                response,
                FederationMeltQuoteMetadata {
                    request_lookup_id: Some(request_lookup_id.clone()),
                    ..FederationMeltQuoteMetadata::default()
                },
            ),
        );
        persist_and_apply_entry(&mint, &entry).await;
    }

    let original_inputs = vec![
        proof_for_runtime(&runtime_config, "paid-lookup-original-input-one"),
        proof_for_runtime(&runtime_config, "paid-lookup-original-input-two"),
    ];
    let original_input_ys = original_inputs
        .ys()
        .expect("paid lookup original input ys");
    let original_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(MeltRequest::new(
                original_quote.clone(),
                original_inputs,
                None,
            ))
            .expect("paid lookup original melt operation"),
    );
    persist_and_apply_entry(&mint, &original_entry).await;
    assert!(mint
        .probe_federation_melt_quote_payment(&original_quote)
        .await
        .expect("start original outgoing payment")
        .is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
    apply_paid_melt_observation_quorum(
        &mint,
        &runtime_config,
        &mut journal,
        &original_quote,
        "paid-lookup-original-preimage",
    )
    .await;

    let duplicate_inputs = vec![
        proof_for_runtime(&runtime_config, "paid-lookup-duplicate-input-one"),
        proof_for_runtime(&runtime_config, "paid-lookup-duplicate-input-two"),
    ];
    let duplicate_input_ys = duplicate_inputs
        .ys()
        .expect("paid lookup duplicate input ys");
    let duplicate_envelope = runtime_config
        .melt_operation(MeltRequest::new(
            duplicate_quote.clone(),
            duplicate_inputs,
            None,
        ))
        .expect("paid lookup duplicate melt operation");
    let duplicate_operation_id = duplicate_envelope
        .operation_id()
        .expect("paid lookup duplicate operation id");
    let duplicate_entry = appended_entry(&mut journal, duplicate_envelope);
    persist_journal_entry(&localstore, &duplicate_entry).await;

    assert!(matches!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&duplicate_entry))
            .await,
        Err(Error::RequestAlreadyPaid)
    ));
    assert_eq!(
        localstore
            .get_melt_quote(&original_quote)
            .await
            .expect("original quote lookup")
            .expect("original quote")
            .state,
        MeltQuoteState::Paid
    );
    assert_eq!(
        localstore
            .get_melt_quote(&duplicate_quote)
            .await
            .expect("duplicate quote lookup")
            .expect("duplicate quote")
            .state,
        MeltQuoteState::Unpaid
    );
    assert_eq!(
        localstore
            .get_proofs_states(&original_input_ys)
            .await
            .expect("original proof states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert_eq!(
        localstore
            .get_proofs_states(&duplicate_input_ys)
            .await
            .expect("duplicate proof states"),
        vec![None, None]
    );
    assert!(localstore
        .get_melt_saga_by_quote_id(&duplicate_quote)
        .await
        .expect("duplicate saga lookup")
        .is_none());
    assert!(localstore
        .get_completed_operation(&super::super::federation_operation_uuid(
            duplicate_operation_id
        ))
        .await
        .expect("duplicate completed-operation lookup")
        .is_none());
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        1,
        "duplicate lookup ID must fail before a second backend payment"
    );
}

#[tokio::test]
async fn apply_federation_melt_keeps_terminal_zero_change_paid_quote_immutable() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let original_proofs = vec![
        proof_for_runtime(&runtime_config, "terminal-paid-zero-change-input-one"),
        proof_for_runtime(&runtime_config, "terminal-paid-zero-change-input-two"),
    ];
    let original_input_ys = original_proofs
        .ys()
        .expect("terminal-paid zero-change input ys");
    let mut zero_change =
        output_for_runtime(&runtime_config, b"terminal-paid zero-change placeholder");
    zero_change.amount = Amount::ZERO;
    let original_request = MeltRequest::new(
        quote.clone(),
        original_proofs,
        Some(vec![zero_change.clone()]),
    );
    let original_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(original_request)
            .expect("terminal-paid zero-change melt operation"),
    );
    persist_and_apply_entry(&mint, &original_entry).await;

    apply_paid_melt_observation_quorum(
        &mint,
        &runtime_config,
        &mut journal,
        &quote,
        "terminal-paid-zero-change-preimage",
    )
    .await;
    let payment_calls_after_paid = make_payment_calls.load(Ordering::Relaxed);
    assert!(
        localstore
            .get_blind_signatures(&[zero_change.blinded_secret])
            .await
            .expect("zero-change signature lookup")[0]
            .is_none(),
        "zero-change placeholder should not be signed or stored"
    );

    let mut tx = localstore
        .begin_transaction()
        .await
        .expect("begin failed-status transaction");
    tx.set_federation_operation_application_state(
        original_entry.operation_id,
        &FederationApplicationState::Failed {},
    )
    .await
    .expect_err("terminal application state must be immutable");
    tx.rollback()
        .await
        .expect("roll back failed-status transaction");

    assert!(!mint
        .apply_accepted_federation_operation(&original_entry)
        .await
        .expect("terminal paid zero-change replay should be a no-op"));
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(original_entry.operation_id)
            .await
            .expect("zero-change replay journal lookup")
            .expect("zero-change replay journal entry")
            .status(),
        FederationOperationStatus::Applied
    );
    assert_eq!(
        localstore
            .get_proofs_states(&original_input_ys)
            .await
            .expect("zero-change replay proof states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        payment_calls_after_paid,
        "terminal paid zero-change replay must not attempt another payment"
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    set_melt_quote_state(&localstore, &quote, MeltQuoteState::Failed).await;

    let (melt_envelope, input_ys) = locally_authored_melt_operation(&runtime_config, |attempt| {
        let inputs = vec![
            proof_for_runtime(
                &runtime_config,
                &format!("failed quote retry {attempt} input one"),
            ),
            proof_for_runtime(
                &runtime_config,
                &format!("failed quote retry {attempt} input two"),
            ),
        ];
        let input_ys = inputs.ys().expect("failed quote retry input ys");
        (MeltRequest::new(quote.clone(), inputs, None), input_ys)
    });
    let melt_entry = appended_entry(&mut journal, melt_envelope);

    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    assert!(mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("failed quote retry payment probe")
        .is_none());
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        1,
        "the observer should attempt the reserved outgoing payment"
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
        melt_quote_op(
            &runtime_config,
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

    let (melt_envelope, (input_ys, change_output)) =
        locally_authored_melt_operation(&runtime_config, |attempt| {
            let inputs = vec![
                proof_for_runtime(
                    &runtime_config,
                    &format!("unknown backend {attempt} input one"),
                ),
                proof_for_runtime(
                    &runtime_config,
                    &format!("unknown backend {attempt} input two"),
                ),
            ];
            let input_ys = inputs.ys().expect("unknown backend input ys");
            let mut change_output = output_for_runtime(&runtime_config, b"unknown backend change");
            change_output.amount = Amount::ZERO;
            (
                MeltRequest::new(quote.clone(), inputs, Some(vec![change_output.clone()])),
                (input_ys, change_output),
            )
        });
    let melt_entry = appended_entry(&mut journal, melt_envelope);
    persist_and_apply_entry(&mint, &melt_entry).await;
    assert!(mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("unknown backend payment probe")
        .is_none());

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
            .status(),
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
            .status(),
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
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let (melt_envelope, input_ys) = locally_authored_melt_operation(&runtime_config, |attempt| {
        let proofs = vec![
            proof_for_runtime(
                &runtime_config,
                &format!("reserve-before-payment-{attempt}-input-one"),
            ),
            proof_for_runtime(
                &runtime_config,
                &format!("reserve-before-payment-{attempt}-input-two"),
            ),
        ];
        let input_ys = proofs.ys().expect("reserve-before-payment input ys");
        (MeltRequest::new(quote.clone(), proofs, None), input_ys)
    });
    *expected_reserve.lock().await = Some(ExpectedMeltReserve {
        quote: quote.clone(),
        input_ys: input_ys.clone(),
    });

    let melt_entry = appended_entry(&mut journal, melt_envelope);
    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
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
        SagaStateEnum::Melt(MeltSagaState::SetupComplete)
    );
    assert_eq!(
        saga.federation_operation_id,
        Some(melt_entry.operation_id.to_string())
    );
    assert_eq!(
        saga.operation_id,
        super::super::federation_operation_uuid(melt_entry.operation_id),
        "federated melt saga IDs must be deterministic so completed-operation rows converge"
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
    assert_eq!(stored_entry.status(), FederationOperationStatus::Applied);

    assert!(mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("reserved melt payment probe")
        .is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("melt saga lookup after payment attempt")
        .expect("melt saga after payment attempt");
    assert_eq!(
        saga.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted)
    );
}

#[tokio::test]
async fn paid_federation_melt_replays_finalizing_cleanup_without_second_payment() {
    let runtime_config = runtime_config();
    let (mint, localstore, expected_reserve, make_payment_calls) =
        federated_test_mint_with_reserve_checking_payment_response(
            runtime_config.clone(),
            MeltQuoteState::Paid,
            2,
        )
        .await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            fake_melt_quote_request(1_000, MeltQuoteState::Paid),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let (melt_envelope, (input_ys, change_output)) =
        locally_authored_melt_operation(&runtime_config, |attempt| {
            let proofs = vec![
                proof_for_runtime(
                    &runtime_config,
                    &format!("paid-finalizing-{attempt}-input-one"),
                ),
                proof_for_runtime(
                    &runtime_config,
                    &format!("paid-finalizing-{attempt}-input-two"),
                ),
                proof_for_runtime(
                    &runtime_config,
                    &format!("paid-finalizing-{attempt}-input-three"),
                ),
            ];
            let input_ys = proofs.ys().expect("paid finalizing input ys");
            let mut change_output =
                output_for_runtime(&runtime_config, b"paid finalizing change output");
            change_output.amount = Amount::ZERO;
            (
                MeltRequest::new(quote.clone(), proofs, Some(vec![change_output.clone()])),
                (input_ys, change_output),
            )
        });
    *expected_reserve.lock().await = Some(ExpectedMeltReserve {
        quote: quote.clone(),
        input_ys: input_ys.clone(),
    });
    let melt_entry = appended_entry(&mut journal, melt_envelope);
    persist_journal_entry(&localstore, &melt_entry).await;

    assert_eq!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&melt_entry))
            .await
            .expect("reserve paid finalizing melt"),
        vec![melt_entry.operation_id]
    );
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    let payment_observation = mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("paid payment probe")
        .expect("paid payment observation");
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
    let payment_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_payment_observation_with_metadata(
            payment_observation.quote,
            payment_observation.method,
            payment_observation.state,
            payment_observation.amount,
            payment_observation.fee_reserve,
            payment_observation.total_spent,
            payment_observation.payment_proof,
            payment_observation.payment_id,
        ),
    );
    persist_journal_entry(&localstore, &payment_entry).await;

    crate::test_helpers::mint::set_fail_for("FEDERATION_MELT_FINALIZE_BEFORE_CLEANUP_COMMIT");
    let result = mint
        .apply_federation_journal_entries(std::slice::from_ref(&payment_entry))
        .await;
    crate::test_helpers::mint::clear_fail_for("FEDERATION_MELT_FINALIZE_BEFORE_CLEANUP_COMMIT");
    assert!(matches!(result, Err(Error::Internal)));
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);

    let paid_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("paid quote lookup after cleanup failure")
        .expect("paid quote after cleanup failure");
    assert_eq!(paid_quote.state, MeltQuoteState::Paid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after cleanup failure"),
        vec![Some(State::Spent), Some(State::Spent), Some(State::Spent)]
    );
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("saga lookup after cleanup failure")
        .expect("finalizing saga after cleanup failure");
    assert_eq!(saga.state, SagaStateEnum::Melt(MeltSagaState::Finalizing));
    let finalization_data = saga
        .finalization_data
        .as_ref()
        .expect("persisted finalization data");
    assert_eq!(
        finalization_data.total_spent,
        Amount::new(2, CurrencyUnit::Sat)
    );
    assert_eq!(
        Some(finalization_data.payment_lookup_id.clone()),
        paid_quote.request_lookup_id
    );
    let federation_operation_id = melt_entry.operation_id.to_string();
    assert_eq!(
        saga.federation_operation_id.as_deref(),
        Some(federation_operation_id.as_str())
    );
    assert_eq!(
        localstore
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("change signature after cleanup failure"),
        vec![None]
    );
    assert!(localstore
        .get_federation_signature_shares_for_output(melt_entry.operation_id, 0)
        .await
        .expect("change share after cleanup failure")
        .is_empty());
    let operation_uuid = super::super::federation_operation_uuid(melt_entry.operation_id);
    assert!(localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed melt after cleanup failure")
        .is_none());
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(payment_entry.operation_id)
            .await
            .expect("payment journal lookup after cleanup failure")
            .expect("payment journal entry after cleanup failure")
            .status(),
        FederationOperationStatus::Accepted
    );

    assert_eq!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&payment_entry))
            .await
            .expect("replay paid finalizing melt"),
        vec![payment_entry.operation_id]
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        1,
        "finalizing replay must not call the payment backend again"
    );
    assert!(localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("saga lookup after replay")
        .is_none());
    assert!(localstore
        .get_blind_signatures(&[change_output.blinded_secret])
        .await
        .expect("change signature after replay")[0]
        .is_some());
    assert_eq!(
        localstore
            .get_federation_signature_shares_for_output(melt_entry.operation_id, 0)
            .await
            .expect("change share after replay")
            .len(),
        1
    );
    let completed_operation = localstore
        .get_completed_operation(&operation_uuid)
        .await
        .expect("completed melt after replay")
        .expect("completed melt operation after replay");
    assert_eq!(
        completed_operation.kind(),
        cdk_common::mint::OperationKind::Melt
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(melt_entry.operation_id)
            .await
            .expect("melt journal lookup after replay")
            .expect("melt journal entry after replay")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn failed_federation_melt_rolls_back_atomically_and_replays() {
    let runtime_config = runtime_config();
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let (quote, input_ys) =
        setup_pending_federated_melt(&mint, &localstore, &runtime_config, &mut journal).await;
    assert!(mint
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("pending melt payment probe")
        .is_none());
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);
    let saga = localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("pending saga lookup")
        .expect("pending saga");

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
            },
        ),
    );
    persist_journal_entry(&localstore, &failed_observation).await;

    crate::test_helpers::mint::set_fail_for("FEDERATION_MELT_ROLLBACK_BEFORE_COMMIT");
    let result = mint
        .apply_federation_journal_entries(std::slice::from_ref(&failed_observation))
        .await;
    crate::test_helpers::mint::clear_fail_for("FEDERATION_MELT_ROLLBACK_BEFORE_COMMIT");
    assert!(matches!(result, Err(Error::Internal)));
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 1);

    let pending_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("quote lookup after rollback failure")
        .expect("quote after rollback failure");
    assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after rollback failure"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
    assert_eq!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("saga lookup after rollback failure")
            .expect("saga after rollback failure")
            .operation_id,
        saga.operation_id
    );
    let melt_request_after_failure = {
        let mut tx = localstore
            .begin_transaction()
            .await
            .expect("melt request read transaction after rollback failure");
        let request = tx
            .get_melt_request_and_blinded_messages(&quote)
            .await
            .expect("melt request after rollback failure");
        tx.rollback()
            .await
            .expect("rollback melt request read transaction after failure");
        request
    };
    assert!(melt_request_after_failure.is_some());
    assert_eq!(
        localstore
            .get_federation_payment_observations(
                FederationPaymentObservationContext::MeltQuote,
                &quote.to_string(),
            )
            .await
            .expect("payment observations after rollback failure")
            .len(),
        1,
        "the accepted observation remains durable for replay"
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(failed_observation.operation_id)
            .await
            .expect("failed observation journal lookup")
            .expect("failed observation journal entry")
            .status(),
        FederationOperationStatus::Accepted
    );

    assert_eq!(
        mint.apply_federation_journal_entries(std::slice::from_ref(&failed_observation))
            .await
            .expect("replay failed melt observation"),
        vec![failed_observation.operation_id]
    );
    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        1,
        "failed-observation replay must not attempt another outgoing payment"
    );
    let unpaid_quote = localstore
        .get_melt_quote(&quote)
        .await
        .expect("quote lookup after rollback replay")
        .expect("quote after rollback replay");
    assert_eq!(unpaid_quote.state, MeltQuoteState::Unpaid);
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states after rollback replay"),
        vec![None, None]
    );
    assert!(localstore
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("saga lookup after rollback replay")
        .is_none());
    let melt_request_after_replay = {
        let mut tx = localstore
            .begin_transaction()
            .await
            .expect("melt request read transaction after replay");
        let request = tx
            .get_melt_request_and_blinded_messages(&quote)
            .await
            .expect("melt request after rollback replay");
        tx.rollback()
            .await
            .expect("rollback melt request read transaction after replay");
        request
    };
    assert!(melt_request_after_replay.is_none());
    assert_eq!(
        localstore
            .get_federation_payment_observations(
                FederationPaymentObservationContext::MeltQuote,
                &quote.to_string(),
            )
            .await
            .expect("payment observations after rollback replay")
            .len(),
        1
    );
    assert_eq!(
        localstore
            .get_federation_journal_entry_by_operation_id(failed_observation.operation_id)
            .await
            .expect("failed observation journal lookup after replay")
            .expect("failed observation journal entry after replay")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn real_policy_melt_reservation_does_not_attempt_payment() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);

    let make_payment_calls = Arc::new(AtomicUsize::new(0));
    let payment_processor = Arc::new(CountingBackendAgnosticPaymentProcessor {
        make_payment_calls: Arc::clone(&make_payment_calls),
        first_payment_started: None,
        release_first_payment: None,
    });
    let (mint, localstore) =
        federated_test_mint_with_payment_processor(runtime_config.clone(), payment_processor).await;
    let quote = QuoteId::new();
    let mut journal = InMemoryFederationJournal::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;
    let input_ys;
    let request = {
        let proofs = vec![
            proof_for_runtime(&runtime_config, "reserved-melt-input-one"),
            proof_for_runtime(&runtime_config, "reserved-melt-input-two"),
        ];
        input_ys = proofs.ys().expect("input ys");
        MeltRequest::new(quote.clone(), proofs, None)
    };
    let operation_id = runtime_config
        .melt_operation(request.clone())
        .expect("melt operation")
        .operation_id()
        .expect("melt operation id");

    let response = mint
        .melt_federation(&request, operation_id)
        .await
        .expect("federation melt reserve");

    assert_eq!(response.state(), MeltQuoteState::Pending);
    assert_eq!(make_payment_calls.load(Ordering::Relaxed), 0);
    assert!(
        localstore
            .get_melt_saga_by_quote_id(&quote)
            .await
            .expect("melt saga lookup")
            .is_some(),
        "reserved melt should keep pending saga for later payment observation"
    );
    assert_eq!(
        localstore
            .get_proofs_states(&input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Pending), Some(State::Pending)]
    );
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
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                updated_at: 0,
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
        melt_quote_op(
            &runtime_config,
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
        0
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
    let completed_operation = localstore
        .get_completed_operation(&super::super::federation_operation_uuid(
            melt_entry.operation_id,
        ))
        .await
        .expect("internal settlement completed operation lookup")
        .expect("internal settlement completed operation");
    assert_eq!(completed_operation.completed_at(), &Some(0));
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

struct FrostOnchainInternalMeltFixture {
    runtime_config: FederationRuntimeConfig,
    mint: Mint,
    localstore: Arc<MintSqliteDatabase>,
    mint_quote: QuoteId,
    melt_quote: QuoteId,
    input_ys: Vec<PublicKey>,
    melt_request: MeltRequest<QuoteId>,
    melt_entry: FederationJournalEntry,
}

async fn frost_onchain_internal_melt_fixture() -> FrostOnchainInternalMeltFixture {
    use cdk_common::bitcoin::Address;
    use cdk_common::federation::{FederationAddressAllocationScope, FederationAddressKeychain};
    use cdk_common::nuts::nut30::{
        MeltQuoteOnchainFeeOption, MeltQuoteOnchainRequest, MeltQuoteOnchainResponse,
        MintQuoteOnchainRequest, MintQuoteOnchainResponse,
    };

    let (runtime_config, application_id) =
        super::address_allocation::runtime_with_address_application();
    let (mint, localstore) = federated_test_mint(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let mint_quote = QuoteId::new();

    let allocation_entry = appended_entry(
        &mut journal,
        runtime_config
            .address_allocation_operation(
                mint_quote.to_string(),
                application_id,
                3,
                FederationAddressKeychain::External,
            )
            .expect("allocation operation"),
    );
    persist_and_apply_entry(&mint, &allocation_entry).await;
    let allocation = mint
        .federation_address_allocation(
            FederationAddressAllocationScope {
                application_id,
                application_key_epoch: 3,
            },
            &mint_quote.to_string(),
        )
        .await
        .expect("allocation lookup")
        .expect("allocation");
    let application = runtime_config
        .cdk_onchain_application()
        .expect("active application");
    let script = runtime_config
        .public_config()
        .frost_wallet
        .as_ref()
        .expect("FROST wallet")
        .derive_cdk_onchain_script(
            runtime_config.public_config(),
            application_id,
            3,
            allocation.keychain,
            allocation.derivation_index,
        )
        .expect("allocated script");
    let address = Address::from_script(script.as_script(), application.network)
        .expect("allocated address")
        .to_string();
    let wallet_key = SecretKey::from_slice(&[24; 32]).expect("wallet key");
    let mint_quote_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: mint_quote.to_string(),
                request_lookup_id: PaymentIdentifier::QuoteId(mint_quote.clone()),
                request: MintQuoteRequest::Onchain(MintQuoteOnchainRequest {
                    unit: CurrencyUnit::Sat,
                    pubkey: wallet_key.public_key(),
                }),
                response: MintQuoteResponse::Onchain(MintQuoteOnchainResponse {
                    quote: mint_quote.to_string(),
                    request: address.clone(),
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    expiry: None,
                    pubkey: wallet_key.public_key(),
                    amount_paid: Amount::ZERO,
                    amount_issued: Amount::ZERO,
                    updated_at: 0,
                }),
            },
        )),
    );
    persist_and_apply_entry(&mint, &mint_quote_entry).await;

    let melt_quote = QuoteId::new();
    let fee_option = MeltQuoteOnchainFeeOption {
        fee_index: 7,
        fee_reserve: Amount::from(1),
        estimated_blocks: 6,
    };
    let melt_quote_entry = appended_entry(
        &mut journal,
        runtime_config.melt_quote_operation(
            melt_quote.clone(),
            MeltQuoteRequest::Onchain(MeltQuoteOnchainRequest {
                request: address.clone(),
                unit: CurrencyUnit::Sat,
                amount: Amount::from(1),
            }),
            MeltQuoteCreateResponse::Onchain(MeltQuoteOnchainResponse {
                quote: melt_quote.clone(),
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                method: PaymentMethod::Known(KnownMethod::Onchain),
                state: MeltQuoteState::Unpaid,
                expiry: 4_000_000_000,
                request: address,
                fee_options: vec![fee_option],
                selected_fee_index: None,
                outpoint: None,
                change: None,
            }),
            FederationMeltQuoteMetadata {
                created_time: 1_700_000_000,
                request_lookup_id: Some(PaymentIdentifier::QuoteId(melt_quote.clone())),
                extra_json: None,
                estimated_blocks: Some(6),
            },
        ),
    );
    persist_and_apply_entry(&mint, &melt_quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "onchain-internal-input-one"),
        proof_for_runtime(&runtime_config, "onchain-internal-input-two"),
    ];
    let input_ys = proofs.ys().expect("input ys");
    let melt_request = MeltRequest::new(melt_quote.clone(), proofs, None).fee_index(7);
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(melt_request.clone())
            .expect("melt operation"),
    );

    FrostOnchainInternalMeltFixture {
        runtime_config,
        mint,
        localstore,
        mint_quote,
        melt_quote,
        input_ys,
        melt_request,
        melt_entry,
    }
}

async fn assert_frost_onchain_internal_melt_finalized(
    fixture: &FrostOnchainInternalMeltFixture,
    mint: &Mint,
) {
    assert_eq!(
        fixture
            .localstore
            .get_mint_quote(&fixture.mint_quote)
            .await
            .expect("mint quote lookup")
            .expect("mint quote")
            .state(),
        MintQuoteState::Paid
    );
    assert_eq!(
        fixture
            .localstore
            .get_melt_quote(&fixture.melt_quote)
            .await
            .expect("melt quote lookup")
            .expect("melt quote")
            .state,
        MeltQuoteState::Paid
    );
    assert_eq!(
        fixture
            .localstore
            .get_proofs_states(&fixture.input_ys)
            .await
            .expect("proof states"),
        vec![Some(State::Spent), Some(State::Spent)]
    );
    assert!(fixture
        .localstore
        .get_melt_saga_by_quote_id(&fixture.melt_quote)
        .await
        .expect("melt saga lookup")
        .is_none());
    let mut tx = fixture.localstore.begin_transaction().await.expect("begin");
    assert!(tx
        .get_melt_request_and_blinded_messages(&fixture.melt_quote)
        .await
        .expect("melt request lookup")
        .is_none());
    tx.rollback().await.expect("rollback read transaction");
    assert!(fixture
        .localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MeltQuote,
            &fixture.melt_quote.to_string(),
        )
        .await
        .expect("melt payment observations")
        .is_empty());
    assert!(fixture
        .localstore
        .get_completed_operation(&super::super::federation_operation_uuid(
            fixture.melt_entry.operation_id,
        ))
        .await
        .expect("completed operation lookup")
        .is_some());
    assert!(mint
        .federation_transaction_state()
        .await
        .expect("transaction state")
        .intent_for_quote(&fixture.melt_quote.to_string())
        .is_none());
    assert_eq!(
        fixture
            .localstore
            .get_federation_journal_entry_by_operation_id(fixture.melt_entry.operation_id)
            .await
            .expect("journal lookup")
            .expect("journal entry")
            .status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn accepted_frost_onchain_internal_melt_finalizes_without_pending_intent() {
    let fixture = frost_onchain_internal_melt_fixture().await;
    persist_and_apply_entry(&fixture.mint, &fixture.melt_entry).await;
    assert_frost_onchain_internal_melt_finalized(&fixture, &fixture.mint).await;
    assert!(!fixture
        .mint
        .apply_accepted_federation_operation(&fixture.melt_entry)
        .await
        .expect("terminal internal melt replay"));
}

#[tokio::test]
async fn failed_frost_onchain_internal_melt_replays_after_restart() {
    let fixture = frost_onchain_internal_melt_fixture().await;
    persist_journal_entry(&fixture.localstore, &fixture.melt_entry).await;
    let response = fixture
        .mint
        .melt_federation(&fixture.melt_request, fixture.melt_entry.operation_id)
        .await
        .expect("commit internal settlement before application status");
    assert_eq!(response.state(), MeltQuoteState::Paid);

    let mut tx = fixture.localstore.begin_transaction().await.expect("begin");
    tx.set_federation_operation_application_state(
        fixture.melt_entry.operation_id,
        &FederationApplicationState::Failed {},
    )
    .await
    .expect("mark interrupted journal application failed");
    tx.commit().await.expect("commit failed application status");

    let restarted = restart_federated_test_mint(
        fixture.runtime_config.clone(),
        Arc::clone(&fixture.localstore),
    )
    .await;
    assert_eq!(
        restarted
            .apply_federation_journal_entries(std::slice::from_ref(&fixture.melt_entry))
            .await
            .expect("replay terminal internal melt"),
        vec![fixture.melt_entry.operation_id]
    );
    assert_frost_onchain_internal_melt_finalized(&fixture, &restarted).await;
}

#[tokio::test]
async fn real_policy_melt_payment_observations_finalize_from_stored_quorum() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
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
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore, make_payment_calls) =
        federated_test_mint_with_counting_payment_processor(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        melt_quote_op(
            &runtime_config,
            quote.clone(),
            pending_melt_quote_request(1_000),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let proofs = vec![
        proof_for_runtime(&runtime_config, "catch-up-paid-input-one"),
        proof_for_runtime(&runtime_config, "catch-up-paid-input-two"),
        proof_for_runtime(&runtime_config, "catch-up-paid-input-three"),
    ];
    let input_ys = proofs.ys().expect("catch-up paid input ys");
    let mut change_output = output_for_runtime(&runtime_config, b"catch-up-paid-change");
    change_output.amount = Amount::ZERO;
    let request = MeltRequest::new(quote.clone(), proofs, Some(vec![change_output.clone()]));
    let melt_entry = appended_entry(
        &mut journal,
        runtime_config
            .melt_operation(request.clone())
            .expect("melt operation envelope"),
    );
    persist_and_apply_entry(&mint, &melt_entry).await;

    assert_eq!(
        make_payment_calls.load(Ordering::Relaxed),
        0,
        "catch-up reservation must not pay during deterministic application"
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
    let completed_operation = localstore
        .get_completed_operation(&super::super::federation_operation_uuid(
            melt_entry.operation_id,
        ))
        .await
        .expect("catch-up paid completed operation lookup")
        .expect("catch-up paid completed operation");
    assert_eq!(completed_operation.completed_at(), &Some(0));

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
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
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

#[tokio::test]
async fn offline_melt_author_cannot_veto_payment_beyond_bounded_lease() {
    let mut configs = runtime_configs();
    let author_config = configs.remove(0);
    let follower_config = configs.remove(0);
    let (author, _author_store) = federated_test_mint(author_config.clone()).await;
    let (follower, follower_store) = federated_test_mint(follower_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_request = fake_melt_quote_request(1_000, MeltQuoteState::Paid);
    let MeltQuoteRequest::Bolt11(bolt11_request) = &quote_request else {
        panic!("bolt11 quote request");
    };
    let quote_entry = appended_entry(
        &mut journal,
        author_config.melt_quote_operation(
            quote.clone(),
            quote_request.clone(),
            test_melt_quote_response(&quote, &quote_request),
            cdk_common::federation::FederationMeltQuoteMetadata {
                created_time: 1_700_000_000,
                request_lookup_id: Some(cdk_common::payment::PaymentIdentifier::PaymentHash(
                    *bolt11_request.request.payment_hash().as_ref(),
                )),
                extra_json: None,
                estimated_blocks: None,
            },
        ),
    );
    persist_and_apply_entry(&author, &quote_entry).await;
    persist_and_apply_entry(&follower, &quote_entry).await;

    let melt_entry = appended_entry(
        &mut journal,
        author_config
            .melt_operation(MeltRequest::new(
                quote.clone(),
                vec![
                    proof_for_runtime(&author_config, "author-veto-input-one"),
                    proof_for_runtime(&author_config, "author-veto-input-two"),
                ],
                None,
            ))
            .expect("melt operation"),
    );
    persist_and_apply_entry(&author, &melt_entry).await;
    persist_and_apply_entry(&follower, &melt_entry).await;
    assert_eq!(
        follower
            .localstore
            .get_melt_quote(&quote)
            .await
            .expect("follower quote lookup")
            .expect("follower quote")
            .state,
        MeltQuoteState::Pending
    );

    // The accepted author is offline: it never probes. While the lease has
    // not expired, the follower still cannot pay.
    assert!(follower
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("follower probe before lease expiry")
        .is_none());

    // Advance the follower's consensus frontier beyond the bounded actor
    // lease with unrelated operations.
    for nonce in 0..8_u64 {
        let filler_entry = appended_entry(
            &mut journal,
            melt_quote_op(
                &author_config,
                QuoteId::new(),
                pending_melt_quote_request(2_000 + nonce),
            ),
        );
        persist_and_apply_entry(&follower, &filler_entry).await;
    }

    // With the author still offline past the bounded lease, the follower must
    // be able to continue the same payment safely and the quote must reach a
    // paid observation instead of remaining vetoed.
    let takeover = follower
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("follower takeover probe");
    assert!(
        takeover.is_some(),
        "the accepted author's offline veto left the quote pending: follower \
         probe returned None"
    );
    let takeover = takeover.expect("takeover observation");
    assert_eq!(takeover.state, MeltQuoteState::Paid);
    assert_eq!(
        takeover.payment_id,
        Some(cdk_common::payment::PaymentIdentifier::PaymentHash(
            *bolt11_request.request.payment_hash().as_ref()
        )),
        "the takeover must continue the same payment under its durable key"
    );
    let saga_after = follower_store
        .get_melt_saga_by_quote_id(&quote)
        .await
        .expect("follower saga lookup")
        .expect("follower saga");
    assert_eq!(
        saga_after.state,
        cdk_common::mint::SagaStateEnum::Melt(cdk_common::mint::MeltSagaState::PaymentAttempted)
    );
}

#[tokio::test]
async fn lease_takeover_after_author_backend_success_keeps_matching_paid_evidence() {
    let mut configs = runtime_configs();
    let author_config = configs.remove(0);
    let follower_config = configs.remove(0);
    let (author, _author_store) = federated_test_mint(author_config.clone()).await;
    let (follower, _follower_store) = federated_test_mint(follower_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let quote_request = fake_melt_quote_request(1_000, MeltQuoteState::Paid);
    let MeltQuoteRequest::Bolt11(bolt11_request) = &quote_request else {
        panic!("bolt11 quote request");
    };
    let expected_payment_id = cdk_common::payment::PaymentIdentifier::PaymentHash(
        *bolt11_request.request.payment_hash().as_ref(),
    );
    let quote_entry = appended_entry(
        &mut journal,
        author_config.melt_quote_operation(
            quote.clone(),
            quote_request.clone(),
            test_melt_quote_response(&quote, &quote_request),
            cdk_common::federation::FederationMeltQuoteMetadata {
                created_time: 1_700_000_000,
                request_lookup_id: Some(expected_payment_id.clone()),
                extra_json: None,
                estimated_blocks: None,
            },
        ),
    );
    persist_and_apply_entry(&author, &quote_entry).await;
    persist_and_apply_entry(&follower, &quote_entry).await;

    let melt_entry = appended_entry(
        &mut journal,
        author_config
            .melt_operation(MeltRequest::new(
                quote.clone(),
                vec![
                    proof_for_runtime(&author_config, "author-success-input-one"),
                    proof_for_runtime(&author_config, "author-success-input-two"),
                ],
                None,
            ))
            .expect("melt operation"),
    );
    persist_and_apply_entry(&author, &melt_entry).await;
    persist_and_apply_entry(&follower, &melt_entry).await;

    // The author pays while it holds the first lease epoch, then crashes
    // before its observation can help the quote finalize.
    let author_observation = author
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("author probe")
        .expect("author paid observation");
    assert_eq!(author_observation.state, MeltQuoteState::Paid);
    assert_eq!(
        author_observation.payment_id,
        Some(expected_payment_id.clone())
    );

    for nonce in 0..8_u64 {
        let filler_entry = appended_entry(
            &mut journal,
            melt_quote_op(
                &author_config,
                QuoteId::new(),
                pending_melt_quote_request(2_000 + nonce),
            ),
        );
        persist_and_apply_entry(&follower, &filler_entry).await;
    }

    // The takeover member continues the same payment under the same durable
    // key, so the paid evidence it reports matches the author's instead of
    // splitting the observation quorum.
    let takeover = follower
        .probe_federation_melt_quote_payment(&quote)
        .await
        .expect("follower takeover probe")
        .expect("follower takeover observation");
    assert_eq!(takeover.state, MeltQuoteState::Paid);
    assert_eq!(takeover.payment_id, Some(expected_payment_id));
}
