#![no_main]

use cdk_common::federation::{
    FederationId, FederationMeltOperation, FederationMeltQuoteMetadata,
    FederationMeltQuoteOperation, FederationMeltQuotePaymentObservation, FederationMintOperation,
    FederationMintQuoteOperation, FederationMintQuotePaymentObservation, FederationMintRequest,
    FederationOperation, FederationOperationEnvelope, FederationOperationId,
    FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationQuarantine, FederationPaymentObservationQuarantineReason,
    FederationPaymentObservationQuarantineRecord, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationSwapOperation, InMemoryFederationJournal,
    MemberId,
};
use cdk_common::nuts::nut00::KnownMethod;
use cdk_common::nuts::nut23::QuoteState;
use cdk_common::nuts::{
    BatchMintRequest, BlindedMessage, CurrencyUnit, MeltQuoteCustomRequest,
    MeltQuoteCustomResponse, MeltQuoteState, MeltRequest, MintQuoteCustomRequest,
    MintQuoteCustomResponse, MintQuoteState, MintRequest, PaymentMethod, Proof, SwapRequest,
};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{MeltQuoteCreateResponse, MeltQuoteRequest, MintQuoteRequest, MintQuoteResponse};
use cdk_fuzz::arbitrary_ext::{AmountArb, BlindedMessageArb, CurrencyUnitArb, ProofArb};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(federation_id) = arbitrary_federation_id(&mut u) else {
        return;
    };
    let Ok(operation) = arbitrary_operation(&mut u) else {
        return;
    };

    let envelope = FederationOperationEnvelope::new(federation_id, operation);
    exercise_conversion_surfaces(&envelope);

    let Ok(canonical) = envelope.canonical_bytes() else {
        return;
    };
    if let Ok(decoded) = FederationOperationEnvelope::from_json_slice(&canonical) {
        exercise_conversion_surfaces(&decoded);
        let _ = envelope.conflicts_with(&decoded);
    }
});

fn exercise_conversion_surfaces(envelope: &FederationOperationEnvelope) {
    let _ = envelope.ensure_supported_version();
    let _ = envelope.ensure_admissible_size();
    let _ = envelope.operation_id();
    let _ = envelope.operation.kind();
    let _ = envelope.operation.quote();
    let _ = envelope.operation.conflict_keys();

    let _ = FederationPaymentObservation::from_operation(&envelope.operation);

    let mut journal = InMemoryFederationJournal::new();
    let Ok(outcome) = journal.append(envelope.clone()) else {
        return;
    };
    let entry = outcome.entry();
    let observation = FederationPaymentObservationRecord::from_journal_entry(entry);
    let quarantine = FederationPaymentObservationQuarantineRecord::from_journal_entry(entry);

    if let Some(record) = observation.as_ref() {
        let _ = record.observation.context();
        let _ = record.observation.quote();
        let _ = record.observation.observer();
        let _ = record.observation.state_label();
        let _ = record.observation.is_terminal_state();
        let _ = record
            .observation
            .matches_payment_evidence(&record.observation);
        let _ = record.observation.quorum([&record.observation], 1);
        let report = FederationPaymentObservationReport::from_records(
            record.observation.context(),
            record.observation.quote(),
            1,
            std::slice::from_ref(record),
        );
        let _ = report.summary();
    }

    if let Some(record) = quarantine.as_ref() {
        let report = FederationPaymentObservationReport::from_records_with_quarantines(
            record.quarantine.context,
            &record.quarantine.quote,
            1,
            &[],
            std::slice::from_ref(record),
        );
        let _ = report.summary();
    }
}

fn arbitrary_operation(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationOperation> {
    Ok(match u.int_in_range(0..=7)? {
        0 => FederationOperation::MintQuote(FederationMintQuoteOperation {
            quote: bounded_string(u, 24)?,
            request_lookup_id: PaymentIdentifier::CustomId(bounded_string(u, 24)?),
            request: mint_quote_request(u)?,
            response: mint_quote_response(u)?,
        }),
        1 => FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
            quote: bounded_string(u, 24)?,
            observer: member_id(u)?,
            method: payment_method(u)?,
            state: mint_quote_state(u)?,
            amount_paid: AmountArb::arbitrary(u)?.into_inner(),
            amount_issued: AmountArb::arbitrary(u)?.into_inner(),
            payment_id: None,
            payment_proof: optional_bounded_string(u, 24)?,
        }),
        2 => FederationOperation::Mint(FederationMintOperation {
            request: mint_request(u)?,
        }),
        3 => FederationOperation::MeltQuote(melt_quote_operation(u)?),
        4 => FederationOperation::MeltQuotePayment(FederationMeltQuotePaymentObservation {
            quote: bounded_string(u, 24)?,
            observer: member_id(u)?,
            method: payment_method(u)?,
            state: melt_quote_state(u)?,
            amount: AmountArb::arbitrary(u)?.into_inner(),
            fee_reserve: AmountArb::arbitrary(u)?.into_inner(),
            payment_id: None,
            payment_proof: optional_bounded_string(u, 24)?,
        }),
        5 => FederationOperation::PaymentObservationQuarantine(
            FederationPaymentObservationQuarantine {
                context: payment_observation_context(u)?,
                quote: bounded_string(u, 24)?,
                reason: payment_observation_quarantine_reason(u)?,
                evidence_operation_ids: operation_ids(u, 4)?,
            },
        ),
        6 => FederationOperation::Melt(FederationMeltOperation {
            request: MeltRequest::new(
                bounded_string(u, 24)?,
                proofs(u, 3)?,
                optional_outputs(u, 3)?,
            ),
        }),
        _ => FederationOperation::Swap(FederationSwapOperation {
            request: SwapRequest::new(proofs(u, 3)?, outputs(u, 3)?),
        }),
    })
}

fn mint_request(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationMintRequest> {
    if u.ratio(1, 2)? {
        Ok(FederationMintRequest::Single(MintRequest {
            quote: bounded_string(u, 24)?,
            outputs: outputs(u, 3)?,
            signature: optional_bounded_string(u, 64)?,
        }))
    } else {
        let quote_count = u.int_in_range(0..=3)?;
        let mut quotes = Vec::with_capacity(quote_count);
        for _ in 0..quote_count {
            quotes.push(bounded_string(u, 24)?);
        }
        let quote_amounts = if u.ratio(1, 2)? {
            let mut amounts = Vec::with_capacity(quote_count);
            for _ in 0..quote_count {
                amounts.push(AmountArb::arbitrary(u)?.into_inner());
            }
            Some(amounts)
        } else {
            None
        };
        let signatures = if u.ratio(1, 2)? {
            let mut signatures = Vec::with_capacity(quote_count);
            for _ in 0..quote_count {
                signatures.push(optional_bounded_string(u, 64)?);
            }
            Some(signatures)
        } else {
            None
        };
        Ok(FederationMintRequest::Batch(BatchMintRequest {
            quotes,
            quote_amounts,
            outputs: outputs(u, 3)?,
            signatures,
        }))
    }
}

fn mint_quote_request(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<MintQuoteRequest> {
    Ok(MintQuoteRequest::Custom {
        method: payment_method(u)?,
        request: MintQuoteCustomRequest {
            amount: if u.ratio(1, 2)? {
                Some(AmountArb::arbitrary(u)?.into_inner())
            } else {
                None
            },
            unit: currency_unit(u)?,
            description: optional_bounded_string(u, 32)?,
            pubkey: None,
            extra: serde_json::Value::Null,
        },
    })
}

fn mint_quote_response(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<MintQuoteResponse<String>> {
    Ok(MintQuoteResponse::Custom {
        method: payment_method(u)?,
        response: MintQuoteCustomResponse {
            quote: bounded_string(u, 24)?,
            request: bounded_string(u, 32)?,
            method: payment_method(u)?,
            amount: if u.ratio(1, 2)? {
                Some(AmountArb::arbitrary(u)?.into_inner())
            } else {
                None
            },
            amount_paid: AmountArb::arbitrary(u)?.into_inner(),
            amount_issued: AmountArb::arbitrary(u)?.into_inner(),
            updated_at: u64::arbitrary(u)?,
            unit: if u.ratio(1, 2)? {
                Some(currency_unit(u)?)
            } else {
                None
            },
            expiry: if u.ratio(1, 2)? {
                Some(u64::arbitrary(u)?)
            } else {
                None
            },
            pubkey: None,
            extra: serde_json::Value::Null,
        },
    })
}

fn melt_quote_request(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<MeltQuoteRequest> {
    Ok(MeltQuoteRequest::Custom(MeltQuoteCustomRequest {
        method: bounded_method_name(u)?,
        request: bounded_string(u, 32)?,
        unit: currency_unit(u)?,
        amount: if u.ratio(1, 2)? {
            Some(AmountArb::arbitrary(u)?.into_inner())
        } else {
            None
        },
        extra: serde_json::Value::Null,
    }))
}

fn melt_quote_operation(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationMeltQuoteOperation> {
    let quote = bounded_string(u, 24)?;
    let request = melt_quote_request(u)?;
    let MeltQuoteRequest::Custom(custom_request) = &request else {
        return Err(libfuzzer_sys::arbitrary::Error::IncorrectFormat);
    };
    let method = PaymentMethod::from(custom_request.method.as_str());
    let response = MeltQuoteCreateResponse::Custom((
        method.clone(),
        MeltQuoteCustomResponse {
            quote: quote.clone(),
            method,
            amount: custom_request.amount.unwrap_or_default(),
            fee_reserve: if u.ratio(1, 2)? {
                Some(AmountArb::arbitrary(u)?.into_inner())
            } else {
                None
            },
            state: melt_quote_state(u)?,
            expiry: u64::arbitrary(u)?,
            payment_preimage: optional_bounded_string(u, 24)?,
            change: None,
            request: Some(custom_request.request.clone()),
            unit: Some(custom_request.unit.clone()),
            extra: serde_json::Value::Null,
        },
    ));
    Ok(FederationMeltQuoteOperation {
        quote,
        request,
        response,
        metadata: FederationMeltQuoteMetadata::default(),
    })
}

fn optional_outputs(
    u: &mut Unstructured<'_>,
    max: usize,
) -> libfuzzer_sys::arbitrary::Result<Option<Vec<BlindedMessage>>> {
    if u.ratio(1, 2)? {
        Ok(Some(outputs(u, max)?))
    } else {
        Ok(None)
    }
}

fn outputs(
    u: &mut Unstructured<'_>,
    max: usize,
) -> libfuzzer_sys::arbitrary::Result<Vec<BlindedMessage>> {
    let count = u.int_in_range(0..=max)?;
    let mut outputs = Vec::with_capacity(count);
    for _ in 0..count {
        outputs.push(BlindedMessageArb::arbitrary(u)?.into_inner());
    }
    Ok(outputs)
}

fn proofs(u: &mut Unstructured<'_>, max: usize) -> libfuzzer_sys::arbitrary::Result<Vec<Proof>> {
    let count = u.int_in_range(0..=max)?;
    let mut proofs = Vec::with_capacity(count);
    for _ in 0..count {
        proofs.push(ProofArb::arbitrary(u)?.into_inner());
    }
    Ok(proofs)
}

fn operation_ids(
    u: &mut Unstructured<'_>,
    max: usize,
) -> libfuzzer_sys::arbitrary::Result<Vec<FederationOperationId>> {
    let count = u.int_in_range(0..=max)?;
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        ids.push(FederationOperationId::from_bytes(<[u8; 32]>::arbitrary(u)?));
    }
    Ok(ids)
}

fn arbitrary_federation_id(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationId> {
    Ok(FederationId::from_bytes(<[u8; 32]>::arbitrary(u)?))
}

fn member_id(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<MemberId> {
    MemberId::new(u.int_in_range(1..=16)?)
        .map_err(|_| libfuzzer_sys::arbitrary::Error::IncorrectFormat)
}

fn currency_unit(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<CurrencyUnit> {
    Ok(CurrencyUnitArb::arbitrary(u)?.into_inner())
}

fn payment_method(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<PaymentMethod> {
    Ok(match u.int_in_range(0..=3)? {
        0 => PaymentMethod::Known(KnownMethod::Bolt11),
        1 => PaymentMethod::Known(KnownMethod::Bolt12),
        2 => PaymentMethod::Known(KnownMethod::Onchain),
        _ => PaymentMethod::Custom(bounded_method_name(u)?),
    })
}

fn payment_observation_context(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationPaymentObservationContext> {
    if u.ratio(1, 2)? {
        Ok(FederationPaymentObservationContext::MintQuote)
    } else {
        Ok(FederationPaymentObservationContext::MeltQuote)
    }
}

fn payment_observation_quarantine_reason(
    u: &mut Unstructured<'_>,
) -> libfuzzer_sys::arbitrary::Result<FederationPaymentObservationQuarantineReason> {
    Ok(match u.int_in_range(0..=2)? {
        0 => FederationPaymentObservationQuarantineReason::ConflictingEvidence,
        1 => FederationPaymentObservationQuarantineReason::CompetingTerminalStates,
        _ => FederationPaymentObservationQuarantineReason::OperatorHalt,
    })
}

fn mint_quote_state(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<MintQuoteState> {
    Ok(match u.int_in_range(0..=2)? {
        0 => QuoteState::Unpaid,
        1 => QuoteState::Paid,
        _ => QuoteState::Issued,
    })
}

fn melt_quote_state(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<MeltQuoteState> {
    Ok(match u.int_in_range(0..=3)? {
        0 => MeltQuoteState::Unpaid,
        1 => MeltQuoteState::Pending,
        2 => MeltQuoteState::Paid,
        _ => MeltQuoteState::Failed,
    })
}

fn optional_bounded_string(
    u: &mut Unstructured<'_>,
    max_len: usize,
) -> libfuzzer_sys::arbitrary::Result<Option<String>> {
    if u.ratio(1, 2)? {
        Ok(Some(bounded_string(u, max_len)?))
    } else {
        Ok(None)
    }
}

fn bounded_method_name(u: &mut Unstructured<'_>) -> libfuzzer_sys::arbitrary::Result<String> {
    let value = bounded_ascii_string(u, 16)?;
    if value.is_empty() {
        Ok("custom".to_string())
    } else {
        Ok(value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect::<String>()
            .to_lowercase())
    }
}

fn bounded_string(
    u: &mut Unstructured<'_>,
    max_len: usize,
) -> libfuzzer_sys::arbitrary::Result<String> {
    let value = bounded_ascii_string(u, max_len)?;
    if value.is_empty() {
        Ok("fuzz".to_string())
    } else {
        Ok(value)
    }
}

fn bounded_ascii_string(
    u: &mut Unstructured<'_>,
    max_len: usize,
) -> libfuzzer_sys::arbitrary::Result<String> {
    let len = u.int_in_range(0..=max_len)?;
    let bytes = u.bytes(len)?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(b'a' + (byte % 26)))
        .collect())
}
