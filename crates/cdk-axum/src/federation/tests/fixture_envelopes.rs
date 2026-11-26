//! Operation envelopes, proofs and journal fixtures.

use cdk_common::database::mint::{
    FederationIndexRange, FederationPaymentObservationSelection, FederationSignatureShareKey,
};

use super::*;

pub(super) async fn catch_up_response_with_materialized_digest(
    state: &MintState,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = test_catch_up_response(consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized catch-up state digest");
    response
}

pub(super) async fn aleph_bft_peer_status_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    member_id: MemberId,
    consensus: &FederationConsensusRuntime,
) -> FederationPeerStatus {
    let mut status = consensus.peer_status_for(public_config.federation_id, member_id);
    for configured_member in public_config.members.keys() {
        status
            .aleph_bft_next_unit_index_by_member
            .entry(*configured_member)
            .or_insert(0);
    }
    let frontier = state
        .mint
        .localstore()
        .get_federation_checkpoint_frontier()
        .await
        .expect("peer checkpoint frontier");
    status.application_digest = Some(frontier.application_digest);
    status.latest_audited_checkpoint_id = frontier.latest_audited_checkpoint_id;
    status.materialized_state_digest = Some(
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT peer status digest"),
    );
    status
}

pub(super) async fn aleph_bft_catch_up_response_with_materialized_digest(
    state: &MintState,
    public_config: &FederationConfig,
    consensus: &FederationConsensusRuntime,
    request: &FederationJournalCatchUpRequest,
) -> FederationJournalCatchUpResponse {
    let mut response = aleph_bft_catch_up_response(public_config, consensus, request);
    response.materialized_state_digest =
        materialized_state_digest(state, consensus.journal().entries())
            .await
            .expect("materialized AlephBFT catch-up state digest");
    response
}

pub(super) fn finalize_test_operation(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> Result<FederationSubmitOperationResponse, FederationError> {
    let status = consensus.status();
    let unit_index = consensus
        .finalized_items()
        .last()
        .map_or(0, |item| item.unit_index.saturating_add(1));
    let unit = FederationAlephBftUnitPayload::new(
        status.federation_id,
        status.local_member,
        status.session,
        unit_index,
        vec![FederationConsensusItem::operation(envelope)],
    )
    .expect("test AlephBFT finalized unit");
    let mut responses = consensus.finalize_unit(unit)?;
    Ok(responses
        .pop()
        .expect("one finalized response for one test operation"))
}

pub(super) fn finalize_test_entry(
    consensus: &mut FederationConsensusRuntime,
    envelope: FederationOperationEnvelope,
) -> FederationJournalEntry {
    finalize_test_operation(consensus, envelope)
        .expect("finalize test journal entry")
        .outcome
        .entry()
        .clone()
}

pub(super) async fn persist_test_journal_entry(state: &MintState, entry: &FederationJournalEntry) {
    let runtime_config = state.mint.federation_config().expect("federation config");
    let db = state.mint.localstore();
    let retained_items = db
        .get_federation_aleph_bft_finalized_items(FederationIndexRange::ALL)
        .await
        .expect("load test AlephBFT finalized items");
    let mut adapter = FederationConsensusRuntime::recover_finalized_items_for_test(
        runtime_config.public_config(),
        runtime_config.member_id(),
        0,
        retained_items.clone(),
    )
    .expect("recover test AlephBFT history");
    let unit = FederationAlephBftUnitPayload::new(
        runtime_config.federation_id(),
        runtime_config.member_id(),
        0,
        retained_items.len() as u64,
        vec![FederationConsensusItem::operation(entry.envelope.clone())],
    )
    .expect("test AlephBFT finalized unit");
    adapter
        .finalize_unit(unit)
        .expect("finalize test journal entry");
    let finalized_entry = adapter
        .journal()
        .entries()
        .last()
        .expect("finalized test journal entry");
    assert_eq!(finalized_entry.operation_id, entry.operation_id);
    let new_items = &adapter.finalized_items()[retained_items.len()..];
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin journal transaction");
    tx.add_federation_aleph_bft_finalized_items(new_items)
        .await
        .expect("persist test AlephBFT finalized item");
    tx.add_federation_journal_entry(finalized_entry)
        .await
        .expect("persist journal entry");
    tx.commit().await.expect("commit journal entry");
}

pub(super) async fn persist_finalized_aleph_bft_history(
    state: &MintState,
    adapter: &FederationConsensusRuntime,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin finalized AlephBFT history transaction");
    tx.add_federation_aleph_bft_finalized_items(adapter.finalized_items())
        .await
        .expect("persist finalized AlephBFT history");
    for entry in adapter.journal().entries() {
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist finalized AlephBFT journal entry");
    }
    tx.commit()
        .await
        .expect("commit finalized AlephBFT history");
}

pub(super) async fn add_blinded_message_conflict(
    state: &MintState,
    quote_id: Option<&QuoteId>,
    output: &BlindedMessage,
) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict transaction");
    tx.add_blinded_messages(
        quote_id,
        std::slice::from_ref(output),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .expect("persist conflicting blinded message");
    tx.commit().await.expect("commit conflict transaction");
}

pub(super) async fn remove_blinded_message_conflict(state: &MintState, output: &BlindedMessage) {
    let mut tx = state
        .mint
        .localstore()
        .begin_transaction()
        .await
        .expect("begin conflict cleanup transaction");
    tx.delete_blinded_messages(&[output.blinded_secret])
        .await
        .expect("remove conflicting blinded message");
    tx.commit().await.expect("commit conflict cleanup");
}

pub(super) fn payment_observation_envelope(
    federation_id: FederationId,
) -> FederationOperationEnvelope {
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: "quote-paid".to_string(),
            observer: MemberId::new(1).expect("member id"),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(21),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some("fake-wallet-paid".to_string()),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize payment observation")
}

pub(super) fn oversized_payment_observation_envelope(
    federation_id: FederationId,
) -> FederationOperationEnvelope {
    let mut envelope = payment_observation_envelope(federation_id);
    let FederationOperation::MintQuotePayment(observation) = &mut envelope.operation else {
        panic!("mint quote payment observation");
    };
    observation.payment_proof =
        Some("x".repeat(cdk::federation::FEDERATION_MAX_OPERATION_ENVELOPE_BYTES));
    envelope
        .authorize(&private_member(1))
        .expect("reauthorize oversized payment observation")
}

pub(super) fn mint_quote_envelope(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_operation(
        quote_id.clone(),
        cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(1),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        }),
        cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(Amount::from(1)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        }),
    )
}

pub(super) fn invalid_mint_quote_transcript_envelope(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationOperationEnvelope {
    let mut envelope = mint_quote_envelope(state, quote_id);
    let FederationOperation::MintQuote(operation) = &mut envelope.operation else {
        panic!("expected mint quote operation");
    };
    if let cdk::mint::MintQuoteResponse::Bolt11(response) = &mut operation.response {
        response.quote = QuoteId::new().to_string();
    }
    envelope
        .authorize(
            state
                .mint
                .federation_config()
                .expect("federation config")
                .private_config(),
        )
        .expect("authorize invalid mint quote transcript")
}

pub(super) fn non_v3_output_for_state(state: &MintState, label: &[u8]) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    BlindedMessage::new(
        Amount::from(1),
        Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        ),
        PublicKey::from(BlsG1PublicKey::hash_to_curve(label)),
    )
}

pub(super) fn invalid_mint_operation_keyset_envelope(
    state: &MintState,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: QuoteId::new().to_string(),
                outputs: vec![non_v3_output_for_state(state, b"invalid route mint output")],
                signature: None,
            }),
        },
    ))
}

pub(super) fn paid_observation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.mint_quote_payment_observation(
        quote_id.clone(),
        PaymentMethod::Known(KnownMethod::Bolt11),
        MintQuoteState::Paid,
        Amount::from(1),
        Amount::ZERO,
        Some(format!("paid-{quote_id}")),
    )
}

pub(super) fn mint_operation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    output_label: &[u8],
) -> (FederationOperationEnvelope, BlindedMessage) {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let output = BlindedMessage::new(
        Amount::from(1),
        keyset.id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(output_label)),
    );
    let envelope = config.operation_envelope(FederationOperation::Mint(
        cdk::federation::FederationMintOperation {
            request: cdk::federation::FederationMintRequest::Single(MintRequest {
                quote: quote_id.to_string(),
                outputs: vec![output.clone()],
                signature: None,
            }),
        },
    ));

    (envelope, output)
}

pub(super) fn mint_payment_observation_envelope_for_member(
    state: &MintState,
    quote_id: &QuoteId,
    observer: u16,
    amount_paid: u64,
    payment_proof: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    FederationOperationEnvelope::new(
        config.federation_id(),
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: quote_id.to_string(),
            observer: member_id(observer),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(amount_paid),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(payment_proof.to_string()),
        }),
    )
    .authorize(&private_member(observer))
    .expect("authorize member payment observation")
}

pub(super) fn mint_output(state: &MintState, quote_id: &QuoteId) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay output {quote_id}").as_bytes(),
        )),
    )
}

pub(super) fn aggregate_secret_for_amount(amount: u64) -> BlsSecretKey {
    match amount {
        1 => BlsSecretKey::from_reduced_bytes(&[11; 32]),
        _ => panic!("missing aggregate secret fixture for amount {amount}"),
    }
}

pub(super) fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
    proof_for_amount_with_aggregate_secret(
        state,
        amount,
        label,
        &aggregate_secret_for_amount(amount),
    )
}

pub(super) fn proof_for_amount_with_aggregate_secret(
    state: &MintState,
    amount: u64,
    label: &str,
    aggregate_secret: &BlsSecretKey,
) -> cdk::nuts::Proof {
    let config = state.mint.federation_config().expect("federation config");
    let keyset = &config.public_config().keysets[0];
    let secret = cdk::secret::Secret::new(format!("federation replay proof {label}"));
    let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes()).mul(aggregate_secret);

    cdk::nuts::Proof::new(
        Amount::from(amount),
        keyset.id,
        secret,
        PublicKey::from(signature),
    )
}

pub(super) async fn proof_from_accepted_output(
    states: &[MintState],
    signature_threshold: usize,
    operation_id: FederationOperationId,
    output_index: u32,
    output: &BlindedMessage,
    secret: cdk::secret::Secret,
) -> cdk::nuts::Proof {
    let mut shares = Vec::with_capacity(signature_threshold);
    for state in states.iter().take(signature_threshold) {
        let records = state
            .mint
            .localstore()
            .get_federation_signature_shares(FederationSignatureShareKey::Output {
                operation_id,
                output_index,
            })
            .await
            .expect("accepted output signature share lookup");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].blinded_secret, output.blinded_secret);
        assert!(records[0].accepted_operation_index.is_some());
        shares.push(records[0].share);
    }
    let signature = aggregate_blind_signature_shares(signature_threshold, &shares)
        .expect("accepted output aggregate signature");

    cdk::nuts::Proof::new(
        output.amount,
        output.keyset_id,
        secret,
        PublicKey::from(signature),
    )
}

pub(super) fn swap_output(state: &MintState, label: &str) -> BlindedMessage {
    let config = state.mint.federation_config().expect("federation config");
    BlindedMessage::new(
        Amount::from(1),
        config.public_config().keysets[0].id,
        PublicKey::from(BlsG1PublicKey::hash_to_curve(
            format!("federation replay swap output {label}").as_bytes(),
        )),
    )
}

pub(super) fn test_melt_quote_operation(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
) -> FederationOperationEnvelope {
    let cdk::MeltQuoteRequest::Bolt11(bolt11) = &request else {
        panic!("federation route tests only construct bolt11 melt quotes");
    };
    let response = cdk::MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
        quote: quote.clone(),
        amount: Amount::from(
            bolt11
                .request
                .amount_milli_satoshis()
                .unwrap_or(1_000)
                .div_ceil(1_000),
        ),
        fee_reserve: Amount::from(1),
        state: MeltQuoteState::Unpaid,
        expiry: 4_000_000_000,
        payment_preimage: None,
        change: None,
        request: Some(bolt11.request.to_string()),
        unit: Some(bolt11.unit.clone()),
        method: PaymentMethod::Known(KnownMethod::Bolt11),
    });
    config.melt_quote_operation(
        quote,
        request,
        response,
        FederationMeltQuoteMetadata::default(),
    )
}

pub(super) fn test_melt_quote_operation_with_lookup_id(
    config: &cdk::mint::FederationRuntimeConfig,
    quote: QuoteId,
    request: cdk::MeltQuoteRequest,
    request_lookup_id: PaymentIdentifier,
) -> FederationOperationEnvelope {
    let envelope = test_melt_quote_operation(config, quote, request);
    let FederationOperation::MeltQuote(mut operation) = envelope.operation else {
        unreachable!("test melt quote helper always creates a melt quote operation");
    };
    operation.metadata.request_lookup_id = Some(request_lookup_id);
    config.operation_envelope(FederationOperation::MeltQuote(operation))
}

pub(super) async fn melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
    melt_quote_with_description(state, amount_msat, "".to_string()).await
}

pub(super) async fn melt_quote_with_description(
    state: &MintState,
    amount_msat: u64,
    description: String,
) -> QuoteId {
    let quote_id = QuoteId::new();
    let request = cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
        request: create_fake_invoice(amount_msat, description),
        unit: CurrencyUnit::Sat,
        options: None,
    });
    let config = state.mint.federation_config().expect("federation config");
    let envelope = test_melt_quote_operation(config, quote_id.clone(), request);
    state
        .submit_federation_envelope(envelope)
        .await
        .expect("submit melt quote through federation consensus");
    quote_id
}

pub(super) fn melt_payment_observation_envelope(
    state: &MintState,
    quote_id: &QuoteId,
    state_observed: MeltQuoteState,
    payment_proof: Option<String>,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::MeltQuotePayment(
        FederationMeltQuotePaymentObservation {
            quote: quote_id.to_string(),
            observer: config.member_id(),
            method: PaymentMethod::BOLT11,
            state: state_observed,
            amount: Amount::from(1),
            fee_reserve: Amount::from(1),
            total_spent: match state_observed {
                MeltQuoteState::Paid => Some(Amount::from(2)),
                _ => None,
            },
            payment_proof,
            payment_id: None,
        },
    ))
}

pub(super) async fn submit_melt_payment_observation_from_backend(
    state: &MintState,
    quote_id: &QuoteId,
) -> FederationPaymentObservationRecord {
    let observer = state
        .mint
        .federation_config()
        .expect("federation config")
        .member_id();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let observed = state
                .federation
                .payment_observer
                .observe_melt_quote(state, quote_id)
                .await
                .expect("probe and submit melt payment observation");
            if observed {
                // The observation may already have paid the quote, which
                // retires it; the history keeps it.
                return state
                    .mint
                    .localstore()
                    .get_federation_payment_observations(
                        FederationPaymentObservationSelection::History {
                            context: FederationPaymentObservationContext::MeltQuote,
                            quote: &quote_id.to_string(),
                        },
                    )
                    .await
                    .expect("melt payment observation lookup")
                    .into_iter()
                    .map(|stored| stored.record)
                    .find(|record| record.observation.observer() == observer)
                    .expect("observer payment record");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("melt payment observation timeout")
}

pub(super) fn matching_melt_payment_observation_envelope(
    state: &MintState,
    evidence: &FederationPaymentObservation,
) -> FederationOperationEnvelope {
    let FederationPaymentObservation::MeltQuote(evidence) = evidence else {
        panic!("expected melt payment evidence");
    };
    let config = state.mint.federation_config().expect("federation config");
    config.operation_envelope(FederationOperation::MeltQuotePayment(
        FederationMeltQuotePaymentObservation {
            observer: config.member_id(),
            ..evidence.clone()
        },
    ))
}

pub(super) fn test_operation_envelope(
    federation_id: FederationId,
    label: &str,
) -> FederationOperationEnvelope {
    let hash = BlsG1PublicKey::hash_to_curve(label.as_bytes()).to_bytes();
    let mut quote_bytes = [0u8; 16];
    quote_bytes.copy_from_slice(&hash[..16]);
    let quote = QuoteId::from(uuid::Uuid::from_bytes(quote_bytes)).to_string();
    FederationOperationEnvelope::new(
        federation_id,
        FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote,
            observer: member_id(1),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            state: MintQuoteState::Paid,
            amount_paid: Amount::from(1),
            amount_issued: Amount::ZERO,
            payment_id: None,
            payment_proof: Some(label.to_string()),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize test operation")
}

pub(super) fn test_operation_envelope_for_state(
    state: &MintState,
    label: &str,
) -> FederationOperationEnvelope {
    let config = state.mint.federation_config().expect("federation config");
    test_operation_envelope_for_config(config.public_config(), label)
}

pub(super) fn test_operation_envelope_for_config(
    config: &FederationConfig,
    label: &str,
) -> FederationOperationEnvelope {
    let hash = BlsG1PublicKey::hash_to_curve(label.as_bytes()).to_bytes();
    let mut quote_bytes = [0u8; 16];
    quote_bytes.copy_from_slice(&hash[..16]);
    let quote_id = QuoteId::from(uuid::Uuid::from_bytes(quote_bytes));
    let quote = quote_id.to_string();
    FederationOperationEnvelope::new(
        config.federation_id,
        FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: quote.clone(),
            request_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            request: cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
                amount: Amount::from(1),
                unit: CurrencyUnit::Sat,
                description: None,
                pubkey: None,
            }),
            response: cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
                quote,
                request: format!("invoice-{label}"),
                amount: Some(Amount::from(1)),
                unit: Some(CurrencyUnit::Sat),
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                amount_paid: Amount::ZERO,
                amount_issued: Amount::ZERO,
                updated_at: 0,
                state: MintQuoteState::Unpaid,
                expiry: Some(10_000),
                pubkey: None,
            }),
        }),
    )
    .authorize(&private_member(1))
    .expect("authorize valid test operation")
}

pub(super) fn restore_request_for_config(config: &FederationConfig, label: &str) -> RestoreRequest {
    RestoreRequest {
        outputs: vec![cdk::nuts::BlindedMessage::new(
            Amount::ZERO,
            config.keysets[0].id,
            PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
        )],
    }
}
