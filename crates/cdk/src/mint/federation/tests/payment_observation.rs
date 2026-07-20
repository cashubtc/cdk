use super::*;

#[test]
fn one_false_bark_result_observer_cannot_form_the_inner_quorum() {
    use cdk_common::federation::{
        FederationBarkActionId, FederationBarkDigest, FederationBarkMemberObservation,
        FederationBarkObservationRecord, FederationBarkObservationStatus, FederationBarkOperation,
        FederationBarkProjectionSnapshot, FederationOperationId, FEDERATION_BARK_OPERATION_VERSION,
    };

    let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action ID");
    let payment_hash = FederationBarkDigest::from_bytes([2; 32]).expect("payment hash");
    let source_scope = FederationBarkDigest::from_bytes([3; 32]).expect("source scope");
    let result_hash = FederationBarkDigest::from_bytes([4; 32]).expect("result hash");
    let observation = |observer: MemberId, operation_index: u64| {
        FederationBarkObservationRecord {
            operation_id: FederationOperationId::from_bytes([operation_index as u8; 32]),
            operation_index,
            operation: FederationBarkOperation::MemberObservation(
                FederationBarkMemberObservation {
                    version: FEDERATION_BARK_OPERATION_VERSION,
                    action_id,
                    quote: Some("quote".to_string()),
                    observer,
                    observation_epoch: operation_index,
                    source_scope,
                    payment_hash: Some(payment_hash),
                    status: FederationBarkObservationStatus::Succeeded,
                    amount: Amount::from(42),
                    fee: Amount::from(7),
                    result_hash,
                },
            ),
        }
    };
    let dishonest = MemberId::new(1).expect("member one");
    let honest = MemberId::new(2).expect("member two");
    let mut projection = FederationBarkProjectionSnapshot {
        observations: vec![observation(dishonest, 1), observation(dishonest, 2)],
        ..FederationBarkProjectionSnapshot::default()
    };
    let key = |status, fee| FederationBarkResultObservationKey {
        action_id,
        status,
        payment_hash,
        source_scope,
        amount: Amount::from(42),
        fee: Amount::from(fee),
        result_hash,
    };

    assert_eq!(
        federation_bark_matching_result_observers(
            &projection,
            key(FederationBarkObservationStatus::Succeeded, 7),
        )
        .len(),
        1,
        "duplicate reports from one member remain one vote",
    );
    assert!(federation_bark_matching_result_observers(
        &projection,
        key(FederationBarkObservationStatus::Succeeded, 6),
    )
    .is_empty());

    projection.observations.push(observation(honest, 3));
    assert_eq!(
        federation_bark_matching_result_observers(
            &projection,
            key(FederationBarkObservationStatus::Succeeded, 7),
        )
        .len(),
        2,
    );
    let mut recovered_failure = observation(MemberId::new(3).expect("member three"), 4);
    let FederationBarkOperation::MemberObservation(failed) = &mut recovered_failure.operation
    else {
        panic!("member observation")
    };
    failed.status = FederationBarkObservationStatus::Failed;
    projection.observations.push(recovered_failure);
    assert_eq!(
        federation_bark_matching_result_observers(
            &projection,
            key(FederationBarkObservationStatus::Failed, 7),
        )
        .len(),
        1,
        "safe failure votes cannot be substituted with success votes",
    );
}

/// Payment backend double whose incoming status returns one fixed,
/// deliberately ordered payment list, to reproduce backend return-order
/// dependence in settlement identity selection.
struct OrderedIncomingPaymentProcessor {
    payments: Vec<WaitPaymentResponse>,
}

#[async_trait]
impl MintPayment for OrderedIncomingPaymentProcessor {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        BackendAgnosticPaymentProcessor.get_settings().await
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .create_incoming_payment_request(options)
            .await
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .get_payment_quote(unit, options)
            .await
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .make_payment(unit, options)
            .await
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Event> + Send>>, Self::Err> {
        BackendAgnosticPaymentProcessor.wait_payment_event().await
    }

    fn is_payment_event_stream_active(&self) -> bool {
        BackendAgnosticPaymentProcessor.is_payment_event_stream_active()
    }

    fn cancel_payment_event_stream(&self) {
        BackendAgnosticPaymentProcessor.cancel_payment_event_stream();
    }

    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Ok(self.payments.clone())
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        BackendAgnosticPaymentProcessor
            .check_outgoing_payment(payment_identifier)
            .await
    }
}

fn ordered_payment(payment_id: &str, amount_sat: u64) -> WaitPaymentResponse {
    WaitPaymentResponse {
        payment_identifier: PaymentIdentifier::CustomId("shared-invoice".to_string()),
        payment_amount: Amount::new(amount_sat * 1_000, CurrencyUnit::Msat),
        payment_id: payment_id.to_string(),
    }
}

#[tokio::test]
async fn mint_payment_observation_identity_is_canonical_across_backend_orderings() {
    let mut configs = runtime_configs();
    let first_config = configs.remove(0);
    let second_config = configs.remove(0);
    let (first_mint, _first_store) = federated_test_mint_with_payment_processor(
        first_config.clone(),
        Arc::new(OrderedIncomingPaymentProcessor {
            payments: vec![
                ordered_payment("payment-a", 60),
                ordered_payment("payment-b", 60),
            ],
        }),
    )
    .await;
    let (second_mint, _second_store) = federated_test_mint_with_payment_processor(
        second_config.clone(),
        Arc::new(OrderedIncomingPaymentProcessor {
            payments: vec![
                ordered_payment("payment-b", 60),
                ordered_payment("payment-a", 60),
            ],
        }),
    )
    .await;

    let mut journal = InMemoryFederationJournal::new();
    let quote_id = QuoteId::new();
    let quote_entry = appended_entry(
        &mut journal,
        first_config.mint_quote_operation(
            quote_id.clone(),
            bolt11_quote_request(100),
            bolt11_quote_response(&quote_id, 100),
        ),
    );
    persist_and_apply_entry(&first_mint, &quote_entry).await;
    persist_and_apply_entry(&second_mint, &quote_entry).await;

    // Both backends report the same two payments in opposite orders. Exact
    // quorum evidence must select one identical settlement identity anyway.
    let first_observation = first_mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("first probe")
        .expect("first observation");
    let second_observation = second_mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("second probe")
        .expect("second observation");

    assert!(
        first_observation.matches_payment_evidence(&second_observation),
        "backend return order must not split settlement identity: {:?} vs {:?}",
        first_observation.payment_id,
        second_observation.payment_id
    );
}

#[tokio::test]
async fn proposer_only_invoice_visibility_cannot_form_payment_quorum() {
    let mut configs = runtime_configs();
    let mut first_config = configs.remove(0);
    let mut second_config = configs.remove(0);
    first_config.public_config.payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    second_config.public_config.payment_observation_policy =
        FederationPaymentObservationPolicy::member_quorum(2);
    let (first_mint, first_store) = federated_test_mint(first_config.clone()).await;
    let (second_mint, second_store) = federated_test_mint(second_config.clone()).await;

    // Member 1 creates a real quote on its own backend; member 2's backend
    // never sees that invoice.
    let backend_response = first_mint
        .get_mint_quote(MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(100),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }))
        .await
        .expect("backend mint quote");
    let quote_id = backend_response.quote().clone();
    let MintQuoteResponse::Bolt11(bolt11_response) = &backend_response else {
        panic!("expected bolt11 quote response");
    };
    let invoice = bolt11_response.request.clone();
    let payment_lookup_id = PaymentIdentifier::PaymentHash(
        *cdk_common::Bolt11Invoice::from_str(&invoice)
            .expect("backend invoice")
            .payment_hash()
            .as_ref(),
    );
    let operation_response = MintQuoteResponse::Bolt11(bolt11_response.clone().into());
    // The fake backend registers the invoice payment in a spawned task.
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    let mut journal = InMemoryFederationJournal::new();
    let quote_entry = appended_entry(
        &mut journal,
        first_config.mint_quote_operation_with_lookup_id(
            quote_id.clone(),
            payment_lookup_id,
            MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(100),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            operation_response,
        ),
    );
    persist_and_apply_entry(&first_mint, &quote_entry).await;
    persist_and_apply_entry(&second_mint, &quote_entry).await;

    let first_observation = first_mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("first probe");
    let second_observation = second_mint
        .probe_federation_mint_quote_payment(&quote_id)
        .await
        .expect("second probe");
    let first_observation =
        first_observation.expect("the proposer must observe its own invoice payment");
    assert!(
        second_observation.is_none(),
        "the other member's independent backend cannot see the proposer's invoice"
    );

    // Even with the proposer's observation accepted everywhere, the
    // two-member quorum can never form and the quote never becomes paid.
    let observation_entry = appended_entry(
        &mut journal,
        first_config.operation_envelope(FederationOperation::MintQuotePayment(first_observation)),
    );
    persist_and_apply_entry(&first_mint, &observation_entry).await;
    persist_and_apply_entry(&second_mint, &observation_entry).await;
    for store in [&first_store, &second_store] {
        let quote = store
            .get_mint_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_ne!(
            quote.state(),
            MintQuoteState::Paid,
            "proposer-only visibility must not form the payment quorum"
        );
    }
}

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

#[test]
fn cumulative_onchain_payment_commitments_allow_ordered_revisions() {
    let runtime_config = runtime_config();
    let quote = QuoteId::new();
    let mut journal = InMemoryFederationJournal::new();
    for (amount, payment_id) in [(10, [1; 32]), (15, [2; 32])] {
        let envelope = runtime_config.operation_envelope(FederationOperation::MintQuotePayment(
            FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: runtime_config.member_id(),
                method: PaymentMethod::Known(KnownMethod::Onchain),
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(amount),
                amount_issued: Amount::ZERO,
                payment_id: Some(PaymentIdentifier::PaymentId(payment_id)),
                payment_proof: None,
            },
        ));
        journal
            .append(envelope)
            .expect("distinct accepted output commitment is a new cumulative revision");
    }
    assert_eq!(journal.entries().len(), 2);
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
async fn mint_payment_observation_without_prior_quote_is_rejected_without_mutation() {
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
    persist_journal_entry(&localstore, &observation_entry).await;

    let error = mint
        .apply_federation_journal_entries(std::slice::from_ref(&observation_entry))
        .await
        .expect_err("observation without an earlier applied quote must be rejected");
    assert!(matches!(error, Error::UnknownQuote));

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
    assert!(observations_before_quote.is_empty());
    let stored_observation_entry = localstore
        .get_federation_journal_entry_by_operation_id(observation_entry.operation_id)
        .await
        .expect("observation journal lookup")
        .expect("observation journal entry");
    assert_eq!(
        stored_observation_entry.status(),
        FederationOperationStatus::Accepted
    );
}

#[tokio::test]
async fn mint_payment_observation_rejects_quote_applied_only_at_later_index() {
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
    persist_journal_entry(&localstore, &observation_entry).await;

    let quote_entry = appended_entry(
        &mut journal,
        runtime_config.mint_quote_operation(
            quote.clone(),
            bolt11_quote_request(4),
            bolt11_quote_response(&quote, 4),
        ),
    );
    persist_and_apply_entry(&mint, &quote_entry).await;

    let error = mint
        .apply_federation_journal_entries(std::slice::from_ref(&observation_entry))
        .await
        .expect_err("a quote at a later index must not authorize an older observation");
    assert!(matches!(error, Error::UnknownQuote));

    let quote_after_rejection = localstore
        .get_mint_quote(&quote)
        .await
        .expect("quote lookup after quote entry")
        .expect("quote after apply");
    assert_eq!(quote_after_rejection.state(), MintQuoteState::Unpaid);
    assert_eq!(
        quote_after_rejection.amount_paid(),
        Amount::new(0, CurrencyUnit::Sat)
    );
    assert!(localstore
        .get_federation_payment_observations(
            FederationPaymentObservationContext::MintQuote,
            &quote.to_string(),
        )
        .await
        .expect("payment observations")
        .is_empty());
    let stored_observation_entry = localstore
        .get_federation_journal_entry_by_operation_id(observation_entry.operation_id)
        .await
        .expect("observation journal lookup")
        .expect("observation journal entry");
    assert_eq!(
        stored_observation_entry.status(),
        FederationOperationStatus::Accepted
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
async fn mint_payment_observation_uses_payment_id_and_unknown_payment_time() {
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
    assert_eq!(stored_quote.payments[0].time, 0);
}
