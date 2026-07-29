fn federation_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn payment_observation_record_from_entry(
    entry: &FederationJournalEntry,
) -> Result<FederationPaymentObservationRecord, Error> {
    FederationPaymentObservationRecord::from_journal_entry(entry).ok_or(Error::Internal)
}

fn federation_payment_identifier_label(payment_id: &PaymentIdentifier) -> String {
    format!("{}:{}", payment_id.kind(), payment_id)
}

fn federation_operation_status_label(status: FederationOperationStatus) -> &'static str {
    match status {
        FederationOperationStatus::Accepted => "accepted",
        FederationOperationStatus::Applying => "applying",
        FederationOperationStatus::Applied => "applied",
        FederationOperationStatus::Rejected => "rejected",
        FederationOperationStatus::Failed => "failed",
    }
}

fn ensure_same_federation_journal_entry_for_retry(
    persisted: &FederationJournalEntry,
    accepted: &FederationJournalEntry,
) -> Result<(), Error> {
    if persisted.index != accepted.index
        || persisted.operation_id != accepted.operation_id
        || persisted.envelope.canonical_bytes()? != accepted.envelope.canonical_bytes()?
        || persisted.conflict_keys != accepted.conflict_keys
        || persisted.previous_order_digest != accepted.previous_order_digest
        || persisted.order_digest != accepted.order_digest
        || persisted.consensus_evidence != accepted.consensus_evidence
    {
        return Err(Error::Custom(format!(
            "federation operation {} has local status {} but does not match the accepted consensus entry",
            persisted.operation_id,
            federation_operation_status_label(persisted.status())
        )));
    }

    Ok(())
}

fn mint_request_from_input(input: MintInput) -> FederationMintRequest {
    match input {
        MintInput::Single(request) => FederationMintRequest::Single(MintRequest {
            quote: request.quote.to_string(),
            outputs: request.outputs,
            signature: request.signature,
        }),
        MintInput::Batch(request) => FederationMintRequest::Batch(BatchMintRequest {
            quotes: request
                .quotes
                .into_iter()
                .map(|quote| quote.to_string())
                .collect(),
            quote_amounts: request.quote_amounts,
            outputs: request.outputs,
            signatures: request.signatures,
        }),
    }
}

fn mint_input_from_federation_request(request: &FederationMintRequest) -> Result<MintInput, Error> {
    match request {
        FederationMintRequest::Single(request) => Ok(MintInput::Single(MintRequest {
            quote: quote_id_from_federation(&request.quote)?,
            outputs: request.outputs.clone(),
            signature: request.signature.clone(),
        })),
        FederationMintRequest::Batch(request) => Ok(MintInput::Batch(BatchMintRequest {
            quotes: request
                .quotes
                .iter()
                .map(|quote| quote_id_from_federation(quote))
                .collect::<Result<Vec<_>, _>>()?,
            quote_amounts: request.quote_amounts.clone(),
            outputs: request.outputs.clone(),
            signatures: request.signatures.clone(),
        })),
    }
}

fn quote_id_from_federation(quote: &str) -> Result<QuoteId, Error> {
    Ok(QuoteId::from_str(quote)?)
}

fn validate_payment_method_name(method: &PaymentMethod) -> Result<(), Error> {
    if method.is_valid_method_name() {
        return Ok(());
    }

    Err(Error::InvalidPaymentMethod)
}

fn mint_quote_from_federation_operation(
    operation: &FederationMintQuoteOperation,
) -> Result<MintQuote, Error> {
    let quote_id = quote_id_from_federation(&operation.quote)?;
    if operation.response.quote() != &operation.quote {
        return Err(Error::UnknownQuote);
    }

    let method = operation.request.payment_method();
    let unit = operation.request.unit();
    let amount = operation
        .request
        .amount()
        .map(|amount| amount.with_unit(unit.clone()));
    let (amount_paid, amount_issued, extra_json) =
        mint_quote_response_amounts(&operation.response, &unit)?;
    let created_time = mint_quote_response_updated_at(&operation.response);

    Ok(MintQuote::new(
        Some(quote_id.clone()),
        operation.response.request().to_string(),
        unit.clone(),
        amount,
        operation.response.expiry().unwrap_or_default(),
        operation.request_lookup_id.clone(),
        operation.request.pubkey(),
        amount_paid,
        amount_issued,
        method,
        created_time,
        created_time,
        Vec::new(),
        Vec::new(),
        extra_json,
    ))
}

fn federation_accepted_order(entry: &FederationJournalEntry) -> u64 {
    // On-chain batching needs only a stable ordering value, not wall-clock time.
    entry.index.saturating_add(1)
}

fn mint_quote_response_updated_at(response: &MintQuoteResponse<String>) -> u64 {
    match response {
        MintQuoteResponse::Bolt11(response) => response.updated_at,
        MintQuoteResponse::Bolt12(response) => response.updated_at,
        MintQuoteResponse::Onchain(response) => response.updated_at,
        MintQuoteResponse::Custom { response, .. } => response.updated_at,
    }
}

fn mint_quote_response_amounts(
    response: &MintQuoteResponse<String>,
    unit: &CurrencyUnit,
) -> Result<
    (
        Amount<CurrencyUnit>,
        Amount<CurrencyUnit>,
        Option<serde_json::Value>,
    ),
    Error,
> {
    match response {
        MintQuoteResponse::Bolt11(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
        MintQuoteResponse::Bolt12(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
        MintQuoteResponse::Onchain(response) => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            None,
        )),
        MintQuoteResponse::Custom { response, .. } => Ok((
            response.amount_paid.with_unit(unit.clone()),
            response.amount_issued.with_unit(unit.clone()),
            Some(response.extra.clone()),
        )),
    }
}

fn ensure_mint_quote_matches(existing: &MintQuote, expected: &MintQuote) -> Result<(), Error> {
    if existing.id != expected.id
        || existing.unit != expected.unit
        || existing.request != expected.request
        || existing.request_lookup_id != expected.request_lookup_id
        || existing.amount != expected.amount
        || existing.expiry != expected.expiry
        || existing.pubkey != expected.pubkey
        || existing.payment_method != expected.payment_method
    {
        return Err(FederationError::OperationStateInvalid {
            reason: "federation mint quote replay found mismatched local quote".to_string(),
        }
        .into());
    }

    Ok(())
}

fn ensure_melt_quote_matches_payment_observation(
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
) -> Result<(), Error> {
    if quote.payment_method != observation.method {
        return Err(Error::InvalidPaymentMethod);
    }

    if quote.amount() != observation.amount.with_unit(quote.unit.clone())
        || quote.fee_reserve() != observation.fee_reserve.with_unit(quote.unit.clone())
    {
        return Err(FederationError::OperationStateInvalid {
            reason: "federation melt payment observation does not match local quote amounts"
                .to_string(),
        }
        .into());
    }

    Ok(())
}

fn ensure_melt_quote_payment_proof_matches(
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
) -> Result<(), Error> {
    if observation.state == cdk_common::MeltQuoteState::Paid
        && observation.payment_proof.is_some()
        && quote.payment_proof != observation.payment_proof
    {
        return Err(FederationError::OperationStateInvalid {
            reason: "federation melt payment observation found mismatched payment proof"
                .to_string(),
        }
        .into());
    }

    Ok(())
}

fn validate_federation_onchain_melt_receipt(
    quote: &MeltQuote,
    melt_operation_id: FederationOperationId,
    receipt: &FederationOnchainMeltPaymentReceipt,
) -> Result<(), Error> {
    let amount_sat = quote.amount().to_sat()?;
    let maximum_fee_sat = quote.fee_reserve().to_sat()?;
    if receipt.quote != quote.id.to_string()
        || receipt.melt_operation_id != melt_operation_id
        || receipt.amount_sat != amount_sat
        || receipt.fee_sat > maximum_fee_sat
        || receipt.outpoint.txid != receipt.txid
    {
        return Err(FederationError::OperationStateInvalid {
            reason: "federation on-chain melt receipt does not match the quote or melt operation"
                .to_string(),
        }
        .into());
    }
    Ok(())
}

fn melt_quote_payment_response(
    operation_id: FederationOperationId,
    quote: &MeltQuote,
    observation: &FederationMeltQuotePaymentObservation,
    onchain_receipt: Option<&FederationOnchainMeltPaymentReceipt>,
) -> Result<MakePaymentResponse, Error> {
    let payment_lookup_id = observation
        .payment_id
        .clone()
        .or_else(|| quote.request_lookup_id.clone())
        .unwrap_or_else(|| {
            PaymentIdentifier::CustomId(format!("federation-melt-observation-{operation_id}"))
        });
    let total_spent = match onchain_receipt {
        Some(receipt) => Amount::new(
            receipt
                .amount_sat
                .checked_add(receipt.fee_sat)
                .ok_or(Error::AmountOverflow)?,
            quote.unit.clone(),
        ),
        None if observation.state == cdk_common::MeltQuoteState::Paid => match observation
            .total_spent
        {
            Some(total_spent) => total_spent.with_unit(quote.unit.clone()),
            None => {
                // Historical accepted observations predate exact settlement
                // accounting. Pure admission rejects this shape for all new
                // operations, but replay must preserve the old deterministic
                // full-reserve result.
                let amount = observation.amount.with_unit(quote.unit.clone());
                let fee_reserve = observation.fee_reserve.with_unit(quote.unit.clone());
                amount
                    .checked_add(&fee_reserve)
                    .map_err(|_| Error::AmountOverflow)?
            }
        },
        None => Amount::ZERO.with_unit(quote.unit.clone()),
    };

    Ok(MakePaymentResponse {
        payment_lookup_id,
        payment_proof: observation.payment_proof.clone(),
        status: observation.state,
        total_spent,
    })
}

fn melt_quote_request_string(request: &MeltQuoteRequest) -> String {
    match request {
        MeltQuoteRequest::Bolt11(request) => request.request.to_string(),
        MeltQuoteRequest::Bolt12(request) => request.request.clone(),
        MeltQuoteRequest::Onchain(request) => request.request.clone(),
        MeltQuoteRequest::Custom(request) => request.request.clone(),
    }
}

fn federation_melt_quote_immutable_terms_match(existing: &MeltQuote, proposed: &MeltQuote) -> bool {
    let fee_selection_matches = match existing.selected_fee_index {
        Some(selected)
            if existing.payment_method
                == PaymentMethod::Known(cdk_common::nut00::KnownMethod::Onchain) =>
        {
            existing
                .fee_options()
                .iter()
                .find(|option| option.fee_index == selected)
                .is_some_and(|option| {
                    existing.fee_reserve() == option.fee_reserve.with_unit(existing.unit.clone())
                        && existing.estimated_blocks == Some(option.estimated_blocks)
                })
        }
        Some(_) => false,
        None => {
            existing.fee_reserve() == proposed.fee_reserve()
                && existing.estimated_blocks == proposed.estimated_blocks
        }
    };

    existing.id == proposed.id
        && existing.unit == proposed.unit
        && existing.request == proposed.request
        && existing.amount() == proposed.amount()
        && fee_selection_matches
        && existing.expiry == proposed.expiry
        && existing.request_lookup_id == proposed.request_lookup_id
        && existing.options == proposed.options
        && existing.created_time == proposed.created_time
        && existing.payment_method == proposed.payment_method
        && existing.extra_json == proposed.extra_json
        && existing.fee_options() == proposed.fee_options()
}

fn federation_payment_identifier_from_label(payment_id: &str) -> PaymentIdentifier {
    match payment_id.split_once(':') {
        Some((kind, identifier)) => PaymentIdentifier::new(kind, identifier)
            .unwrap_or_else(|_| PaymentIdentifier::CustomId(payment_id.to_string())),
        None => PaymentIdentifier::CustomId(payment_id.to_string()),
    }
}

fn federation_share_signatures(share_set: FederationBlindSignatureShareSet) -> Vec<BlindSignature> {
    share_set
        .shares
        .into_iter()
        .map(|share| BlindSignature {
            amount: share.amount,
            keyset_id: share.keyset_id,
            c: PublicKey::from(share.share.signature()),
            dleq: None,
        })
        .collect()
}

fn validate_federation_mint_quotes(
    quotes: &[database::mint::Acquired<MintQuote>],
) -> Result<(PaymentMethod, CurrencyUnit), Error> {
    let first = quotes.first().ok_or(Error::UnknownQuote)?;
    let payment_method = first.payment_method.clone();
    let unit = first.unit.clone();

    for quote in quotes {
        if quote.payment_method != payment_method {
            return Err(Error::InvalidPaymentMethod);
        }
        if quote.unit != unit {
            return Err(Error::UnitMismatch);
        }
    }

    Ok((payment_method, unit))
}

fn validate_federation_mint_quote_signatures(
    input: &MintInput,
    quotes: &[database::mint::Acquired<MintQuote>],
) -> Result<(), Error> {
    match input {
        MintInput::Single(request) => {
            let quote = quotes.first().ok_or(Error::UnknownQuote)?;
            match quote.pubkey.as_ref() {
                Some(pubkey) => request
                    .verify_signature(*pubkey)
                    .map_err(|_| Error::SignatureMissingOrInvalid),
                None if request.signature.is_some() => Err(Error::SignatureMissingOrInvalid),
                None => Ok(()),
            }
        }
        MintInput::Batch(request) => {
            for (index, quote) in quotes.iter().enumerate() {
                let signature = request
                    .signatures
                    .as_ref()
                    .and_then(|signatures| signatures.get(index))
                    .and_then(Option::as_ref);

                match quote.pubkey.as_ref() {
                    Some(pubkey) => {
                        let signature = signature.ok_or(Error::SignatureMissingOrInvalid)?;
                        let quote_id = request
                            .quotes
                            .get(index)
                            .ok_or(Error::SignatureMissingOrInvalid)?;
                        request
                            .verify_quote_signature(quote_id, signature, pubkey)
                            .map_err(|_| Error::SignatureMissingOrInvalid)?;
                    }
                    None if signature.is_some() => return Err(Error::SignatureMissingOrInvalid),
                    None => {}
                }
            }

            Ok(())
        }
    }
}

fn validate_federation_mint_quote_states_for_signing(
    quotes: &[database::mint::Acquired<MintQuote>],
    expected_amounts: &[Amount<CurrencyUnit>],
    signatures_present: usize,
    expected_signature_count: usize,
) -> Result<(), Error> {
    if quotes.len() != expected_amounts.len() {
        return Err(Error::Internal);
    }

    for (quote, amount_issued) in quotes.iter().zip(expected_amounts) {
        match quote.state() {
            MintQuoteState::Paid => {}
            MintQuoteState::Issued => {
                if quote.amount_issued() < amount_issued.clone() {
                    return Err(Error::IssuedQuote);
                }
                if signatures_present != expected_signature_count {
                    return Err(Error::IssuedQuote);
                }
            }
            MintQuoteState::Unpaid => {
                return Err(Error::UnpaidQuote);
            }
        }
    }

    Ok(())
}

fn expected_federation_mint_amounts(
    input: &MintInput,
    quotes: &[database::mint::Acquired<MintQuote>],
    outputs_amount: Amount<CurrencyUnit>,
) -> Result<Vec<Amount<CurrencyUnit>>, Error> {
    match input {
        MintInput::Single(_) => Ok(vec![outputs_amount]),
        MintInput::Batch(request) => quotes
            .iter()
            .enumerate()
            .map(|(index, quote)| {
                if quote.state() == MintQuoteState::Unpaid {
                    return Err(Error::UnpaidQuote);
                }
                if quote.state() == MintQuoteState::Issued {
                    return Ok(quote.amount_issued());
                }

                match &request.quote_amounts {
                    Some(amounts) => amounts
                        .get(index)
                        .cloned()
                        .map(|amount| amount.with_unit(quote.unit.clone()))
                        .ok_or(Error::TransactionUnbalanced(0, 0, 0)),
                    None => Ok(quote.amount_mintable()),
                }
            })
            .collect(),
    }
}

fn mint_quote_state_label(state: MintQuoteState) -> &'static str {
    match state {
        MintQuoteState::Unpaid => "unpaid",
        MintQuoteState::Paid => "paid",
        MintQuoteState::Issued => "issued",
    }
}

#[cfg(all(test, feature = "prometheus"))]
mod performance_instrumentation_tests {
    use std::collections::HashSet;

    use super::FederationProofVerificationContext;

    #[test]
    fn proof_verification_contexts_have_distinct_fixed_metric_labels() {
        let contexts = [
            FederationProofVerificationContext::SwapBuilder,
            FederationProofVerificationContext::SwapAdmission,
            FederationProofVerificationContext::SwapApply,
            FederationProofVerificationContext::MeltBuilder,
            FederationProofVerificationContext::MeltAdmission,
            FederationProofVerificationContext::MeltApply,
        ];
        let mut work = HashSet::new();
        let mut operations = HashSet::new();

        for context in contexts {
            let (context_work, context_operation) = context.performance_labels();
            assert!(work.insert(context_work));
            assert!(operations.insert(context_operation));
        }

        assert_eq!(work.len(), contexts.len());
        assert_eq!(operations.len(), contexts.len());
    }
}

#[cfg(test)]
mod tests;
