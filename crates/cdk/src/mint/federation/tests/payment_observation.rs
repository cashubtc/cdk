use super::*;

fn mint_payment_observation_envelope(
    runtime_config: &FederationRuntimeConfig,
    quote: &QuoteId,
    observer: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
        FederationMintQuotePaymentObservation {
            quote: quote.to_string(),
            observer: MemberId::new(observer).expect("observer"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
        },
    ))
}

#[test]
fn runtime_config_builds_mint_payment_observation_operations() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let paid = runtime_config.mint_quote_payment_observation(
        quote.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(10),
        Amount::ZERO,
        Some("settled".to_string()),
    );

    assert_eq!(paid.federation_id, runtime_config.federation_id());
    assert!(matches!(
        paid.operation,
        FederationOperation::MintQuotePayment(_)
    ));

    let mut journal = InMemoryFederationJournal::new();
    journal.append(paid).expect("append paid");
    let conflicting_paid = runtime_config.mint_quote_payment_observation(
        quote,
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(10),
        Amount::ZERO,
        Some("different-settlement".to_string()),
    );

    assert!(matches!(
        journal.append(conflicting_paid),
        Err(FederationError::ConflictingOperation { .. })
    ));
}

#[tokio::test]
async fn real_policy_allows_quorum_despite_minority_disagreement() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(10),
            bolt11_quote_response(&quote, 10),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let first_observation = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 1, 10, "settled"),
    );
    persist_and_apply_entry(&mint, &first_observation).await;
    let quote_after_single = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_single.state(), MintQuoteState::Unpaid);
    assert_eq!(
        quote_after_single.amount_paid(),
        Amount::new(0, CurrencyUnit::Sat)
    );

    let observations_after_single = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("single observation rows");
    assert_eq!(observations_after_single.len(), 1);

    let conflicting_third = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 3, 11, "different-settlement"),
    );
    persist_and_apply_entry(&mint, &conflicting_third).await;
    let quote_after_minority_disagreement = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(
        quote_after_minority_disagreement.state(),
        MintQuoteState::Unpaid
    );

    let matching_second = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 2, 10, "settled"),
    );
    persist_and_apply_entry(&mint, &matching_second).await;
    let quote_after_quorum = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(quote_after_quorum.state(), MintQuoteState::Paid);
    assert_eq!(
        quote_after_quorum.amount_paid(),
        Amount::new(10, CurrencyUnit::Sat)
    );

    let observations_after_conflict = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("conflicting observation rows");
    assert_eq!(observations_after_conflict.len(), 3);
    assert_eq!(
        observations_after_conflict[1].observation.observer(),
        MemberId::new(3).expect("member three")
    );
}

#[tokio::test]
async fn mint_payment_observation_before_quote_materializes_after_quote_apply() {
    let runtime_config = runtime_config_with_payment_policy(
        FederationPaymentObservationPolicy::development_single_observation_for_testing(),
    );
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let observation_entry = appended_entry(
        &mut journal,
        mint_payment_observation_envelope(&runtime_config, &quote, 1, 4, "settled-before-quote"),
    );
    persist_and_apply_entry(&mint, &observation_entry).await;

    assert!(localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup before quote entry")
        .is_none());
    let observations_before_quote = localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("stored observation before quote");
    assert_eq!(observations_before_quote.len(), 1);
    let stored_observation_entry = localstore
        .get_federation_journal_entry_by_operation_id(observation_entry.operation_id)
        .await
        .expect("observation journal lookup")
        .expect("observation journal entry");
    assert_eq!(
        stored_observation_entry.status(),
        FederationOperationStatus::Applied
    );

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(4),
            bolt11_quote_response(&quote, 4),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let quote_after_apply = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup after quote entry")
        .expect("quote after replay");
    assert_eq!(quote_after_apply.state(), MintQuoteState::Paid);
    assert_eq!(
        quote_after_apply.amount_paid(),
        Amount::new(4, CurrencyUnit::Sat)
    );
    let stored_quote_entry = localstore
        .get_federation_journal_entry_by_operation_id(quote_entry.operation_id)
        .await
        .expect("quote journal lookup")
        .expect("quote journal entry");
    assert_eq!(
        stored_quote_entry.status(),
        FederationOperationStatus::Applied
    );
}

#[tokio::test]
async fn mint_payment_observation_apply_error_rolls_back_observation_and_status() {
    let runtime_config = runtime_config_with_payment_policy(
        FederationPaymentObservationPolicy::development_single_observation_for_testing(),
    );
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(4),
            bolt11_quote_response(&quote, 4),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let observation_entry = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: runtime_config.member_id(),
                method: PaymentMethod::Known(KnownMethod::Onchain),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(4),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: Some("wrong-method".to_string()),
            },
        )),
    );
    persist_journal_entry(&localstore, &observation_entry).await;

    let error = mint
        .apply_federation_journal_entries(std::slice::from_ref(&observation_entry))
        .await
        .expect_err("payment-method mismatch must reject apply");
    assert!(matches!(error, Error::InvalidPaymentMethod));
    assert!(localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("payment observations")
        .is_empty());
    let stored_entry = localstore
        .get_federation_journal_entry_by_operation_id(observation_entry.operation_id)
        .await
        .expect("journal lookup")
        .expect("journal entry");
    assert_eq!(stored_entry.status(), FederationOperationStatus::Accepted);
}

#[tokio::test]
async fn mint_payment_observation_uses_payment_id_and_consensus_logical_time() {
    let policy = FederationPaymentObservationPolicy::member_quorum(2);
    let runtime_config = runtime_config_with_payment_policy(policy);
    let (mint, localstore) =
        federated_test_mint_with_backend_agnostic_payment(runtime_config.clone()).await;
    let mut journal = InMemoryFederationJournal::new();
    let quote = QuoteId::new();
    let payment_id = PaymentIdentifier::CustomId("metadata-payment".to_string());

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(7),
            bolt11_quote_response(&quote, 7),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let first_observation = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: MemberId::new(1).expect("member one"),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(7),
                amount_issued: Amount::ZERO,
                payment_id: Some(payment_id.clone()),
                payment_proof: None,
            },
        )),
    );
    persist_and_apply_entry(&mint, &first_observation).await;

    let matching_second = appended_entry(
        &mut journal,
        runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: MemberId::new(2).expect("member two"),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(7),
                amount_issued: Amount::ZERO,
                payment_id: Some(payment_id.clone()),
                payment_proof: None,
            },
        )),
    );
    persist_and_apply_entry(&mint, &matching_second).await;

    let stored_quote = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup")
        .expect("quote");
    assert_eq!(stored_quote.state(), MintQuoteState::Paid);
    assert_eq!(stored_quote.payments.len(), 1);
    assert_eq!(
        stored_quote.payments[0].payment_id,
        "custom:metadata-payment"
    );
    assert_eq!(stored_quote.payments[0].time, 3);
}
