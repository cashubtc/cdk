use cdk_common::mint::{MeltFinalizationData, MeltSagaState, SagaStateEnum};
use cdk_common::util::unix_time;
use cdk_common::{database, MeltQuoteState, QuoteId, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{MeltQuote, Mint, CDK_MINT_PRIMARY_NAMESPACE};

const RESOLUTION_NAMESPACE: &str = "melt_resolutions";

/// An operator's authoritative decision for a specific melt operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteResolution {
    /// Quote being resolved.
    pub quote_id: QuoteId,
    /// Operation ID obtained during inspection; prevents resolving a later retry.
    pub operation_id: Uuid,
    /// Recovery stage observed during inspection.
    pub expected_saga_state: MeltSagaState,
    /// Operator's explanation and external evidence, retained in the audit record.
    pub reason: String,
    /// Authoritative payment outcome to apply.
    pub action: MeltQuoteResolutionAction,
}

/// Manual outcomes; neither action sends or cancels a backend payment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeltQuoteResolutionAction {
    /// Finalize using the actual payment result, in the quote's currency unit.
    Finalize(MeltFinalizationData),
    /// Release reservations after the operator has stopped all backend retries.
    Compensate {
        /// The operator confirms payment failed and cannot subsequently complete.
        payment_failure_confirmed: bool,
    },
}

/// Failure to validate or apply an operator resolution.
#[derive(Debug, thiserror::Error)]
pub enum MeltQuoteResolutionError {
    /// Invalid or incomplete operator input.
    #[error("Invalid melt resolution: {0}")]
    InvalidRequest(String),
    /// The operation is busy, changed, or conflicts with the requested outcome.
    #[error("Melt resolution conflict: {0}")]
    Conflict(String),
    /// No quote exists with this identifier.
    #[error("Unknown melt quote")]
    UnknownQuote,
    /// Database failure.
    #[error(transparent)]
    Database(#[from] database::Error),
    /// Audit serialization failure.
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    /// Finalization or compensation failed; a committed decision remains recoverable.
    #[error(transparent)]
    Recovery(#[from] crate::Error),
}

#[derive(Debug, Serialize, Deserialize)]
struct ResolutionRecord {
    request: MeltQuoteResolution,
    accepted_at: u64,
}

impl Mint {
    /// Applies an operator-verified payment outcome through normal melt cleanup.
    ///
    /// This does not query, pay, or cancel the backend. The caller must establish
    /// the outcome externally. Compensation requires that no payment or retry can
    /// still complete. The decision and reason are persisted atomically with the
    /// recovery handoff. Retrying the identical request resumes that decision;
    /// changing it or targeting a superseding operation fails with a conflict.
    #[tracing::instrument(skip_all)]
    pub async fn resolve_melt_quote(
        &self,
        request: MeltQuoteResolution,
    ) -> Result<MeltQuote, MeltQuoteResolutionError> {
        use MeltQuoteResolutionError::{Conflict, InvalidRequest, UnknownQuote};

        if request.reason.trim().is_empty() || request.reason.len() > 4096 {
            return Err(InvalidRequest(
                "reason must contain 1 to 4096 bytes".to_owned(),
            ));
        }
        if matches!(
            request.action,
            MeltQuoteResolutionAction::Compensate {
                payment_failure_confirmed: false
            }
        ) {
            return Err(InvalidRequest(
                "confirm payment failed and cannot still complete".to_owned(),
            ));
        }
        let quote_lock = self.melt_quote_lock(&request.quote_id).await;
        let _guard = quote_lock.try_lock_owned().map_err(|_| {
            Conflict(
                "quote is busy; inspect it again after the active operation finishes".to_owned(),
            )
        })?;
        let key = request.operation_id.to_string();
        let mut tx = self.localstore.begin_transaction().await?;
        // Match the quote-before-saga lock order used by finalization/rollback.
        let quote = tx
            .lock_melt_quote_and_related(&request.quote_id)
            .await?
            .target
            .ok_or(UnknownQuote)?
            .inner();
        let mut saga = tx.get_saga_for_update(&request.operation_id).await?;
        let recorded = tx
            .kv_read(CDK_MINT_PRIMARY_NAMESPACE, RESOLUTION_NAMESPACE, &key)
            .await?
            .map(|bytes| serde_json::from_slice::<ResolutionRecord>(&bytes))
            .transpose()?;
        if let Some(record) = &recorded {
            if record.request != request {
                return Err(Conflict(
                    "this operation already has a different operator decision".to_owned(),
                ));
            }
        }
        let target_state = match request.action {
            MeltQuoteResolutionAction::Finalize(_) => MeltQuoteState::Paid,
            MeltQuoteResolutionAction::Compensate { .. } => MeltQuoteState::Unpaid,
        };
        let Some(ref mut saga) = saga else {
            if recorded.is_some() && quote.state == target_state {
                tx.rollback().await?;
                if self
                    .localstore
                    .get_melt_saga_by_quote_id(&quote.id)
                    .await?
                    .is_some()
                {
                    return Err(Conflict(
                        "quote now belongs to another operation".to_owned(),
                    ));
                }
                return Ok(quote);
            }
            return Err(Conflict(
                "operation is missing or already completed; inspect the quote again".to_owned(),
            ));
        };
        if saga.quote_id.as_deref() != Some(quote.id.to_string().as_str()) {
            return Err(Conflict(
                "operation does not belong to this quote".to_owned(),
            ));
        }
        if recorded.is_none()
            && saga.state != SagaStateEnum::Melt(request.expected_saga_state.clone())
        {
            return Err(Conflict(
                "recovery stage changed; inspect the quote again".to_owned(),
            ));
        }
        match &request.action {
            MeltQuoteResolutionAction::Finalize(payment) => {
                if payment.total_spent.unit() != &quote.unit || payment.total_spent < quote.amount()
                {
                    return Err(InvalidRequest(
                        "total spent must use the quote unit and cover its amount".to_owned(),
                    ));
                }
                if payment.payment_lookup_id.to_string().is_empty()
                    || payment
                        .payment_proof
                        .as_ref()
                        .is_some_and(|proof| proof.is_empty())
                {
                    return Err(InvalidRequest(
                        "payment lookup ID and supplied proof must not be empty".to_owned(),
                    ));
                }
                if !matches!(quote.state, MeltQuoteState::Pending | MeltQuoteState::Paid)
                    || matches!(
                        saga.state,
                        SagaStateEnum::Melt(MeltSagaState::PaymentFailed) | SagaStateEnum::Swap(_)
                    )
                {
                    return Err(Conflict(
                        "operation cannot be finalized from its current state".to_owned(),
                    ));
                }
                if saga.state == SagaStateEnum::Melt(MeltSagaState::Finalizing) {
                    if saga.finalization_data.as_ref() != Some(payment) {
                        return Err(Conflict(
                            "payment details differ from the durable finalization result"
                                .to_owned(),
                        ));
                    }
                } else if recorded.is_some() {
                    return Err(Conflict(
                        "recorded finalization no longer owns this operation".to_owned(),
                    ));
                }
                if quote.state == MeltQuoteState::Paid
                    && (quote.request_lookup_id.as_ref() != Some(&payment.payment_lookup_id)
                        || quote.payment_proof != payment.payment_proof)
                {
                    return Err(Conflict(
                        "payment details differ from the paid quote".to_owned(),
                    ));
                }
            }
            MeltQuoteResolutionAction::Compensate { .. } => {
                if !(quote.state == MeltQuoteState::Pending
                    || (recorded.is_some() && quote.state == MeltQuoteState::Unpaid))
                    || !matches!(
                        saga.state,
                        SagaStateEnum::Melt(
                            MeltSagaState::SetupComplete
                                | MeltSagaState::PaymentAttempted
                                | MeltSagaState::PaymentPending
                                | MeltSagaState::PaymentFailed
                        )
                    )
                {
                    return Err(Conflict(
                        "paid or finalizing operations cannot be compensated".to_owned(),
                    ));
                }
                if recorded.is_some()
                    && saga.state != SagaStateEnum::Melt(MeltSagaState::PaymentFailed)
                {
                    return Err(Conflict(
                        "recorded compensation no longer owns this operation".to_owned(),
                    ));
                }
            }
        }
        // Validate setup artifacts before committing an irreversible recovery decision.
        let melt_request = tx.get_melt_request_and_blinded_messages(&quote.id).await?;
        let input_ys = tx
            .get_proof_ys_by_operation_id(&request.operation_id)
            .await?;
        // Shared rollback may retain its saga if best-effort saga deletion
        // fails after reservations are removed. Finish only that recorded cleanup.
        if recorded.is_some()
            && target_state == MeltQuoteState::Unpaid
            && quote.state == MeltQuoteState::Unpaid
        {
            if melt_request.is_some() || !input_ys.is_empty() {
                return Err(Conflict(
                    "compensated operation still has setup artifacts".to_owned(),
                ));
            }
            tx.delete_saga(&request.operation_id).await?;
            tx.commit().await?;
            return Ok(quote);
        }
        if quote.state == MeltQuoteState::Pending {
            let info = melt_request.ok_or_else(|| {
                Conflict("melt request is missing; manual database repair is required".to_owned())
            })?;
            if input_ys.is_empty() {
                return Err(Conflict("reserved inputs are missing".to_owned()));
            }
            if let MeltQuoteResolutionAction::Finalize(payment) = &request.action {
                let available = info
                    .inputs_amount
                    .checked_sub(&info.inputs_fee)
                    .map_err(crate::Error::from)?;
                if payment.total_spent > available {
                    return Err(InvalidRequest(
                        "total spent exceeds the reserved inputs after fees".to_owned(),
                    ));
                }
            }
            let proofs = tx.get_proofs(&input_ys).await?;
            if proofs.state != State::Pending || proofs.len() != input_ys.len() {
                return Err(Conflict(
                    "inputs are no longer fully reserved for this operation".to_owned(),
                ));
            }
        }
        if recorded.is_none() {
            let record = ResolutionRecord {
                request: request.clone(),
                accepted_at: unix_time(),
            };
            if !tx
                .kv_write_if_absent(
                    CDK_MINT_PRIMARY_NAMESPACE,
                    RESOLUTION_NAMESPACE,
                    &key,
                    &serde_json::to_vec(&record)?,
                )
                .await?
            {
                return Err(Conflict(
                    "another resolution was recorded concurrently".to_owned(),
                ));
            }
            match &request.action {
                MeltQuoteResolutionAction::Finalize(payment) => {
                    tx.update_acquired_saga_with_finalization_data(
                        saga,
                        SagaStateEnum::Melt(MeltSagaState::Finalizing),
                        Some(payment),
                    )
                    .await?;
                }
                MeltQuoteResolutionAction::Compensate { .. } => {
                    tx.update_acquired_saga(
                        saga,
                        SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
                    )
                    .await?;
                }
            }
        }
        let saga = (**saga).clone();
        tx.commit().await?;
        tracing::warn!(quote_id = %request.quote_id, operation_id = %request.operation_id,
            outcome = %target_state, reason = %request.reason, "operator melt resolution recorded");
        match &request.action {
            MeltQuoteResolutionAction::Finalize(payment) => {
                super::melt::shared::finalize_melt_quote(
                    self,
                    &self.localstore,
                    &self.pubsub_manager,
                    &quote,
                    payment.total_spent.clone(),
                    payment.payment_proof.clone(),
                    &payment.payment_lookup_id,
                    Some(request.operation_id),
                    payment.bolt12_payer_proof_inputs.clone(),
                )
                .await?;
            }
            MeltQuoteResolutionAction::Compensate { .. } => {
                let mut quote = quote;
                super::saga_recovery::recover_recorded_payment_failure(
                    &saga,
                    &mut quote,
                    &self.localstore,
                    &self.pubsub_manager,
                )
                .await?;
            }
        }
        let quote = self
            .localstore
            .get_melt_quote(&request.quote_id)
            .await?
            .ok_or(UnknownQuote)?;
        if quote.state != target_state
            || self
                .localstore
                .get_melt_saga_by_quote_id(&request.quote_id)
                .await?
                .is_some()
        {
            return Err(Conflict(
                "resolution was recorded but cleanup is incomplete; retry the identical request"
                    .to_owned(),
            ));
        }
        Ok(quote)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::mint::Saga;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::{
        Amount, CurrencyUnit, MeltQuoteBolt11Request, MeltRequest, PaymentMethod, PreMintSecrets,
        ProofsMethods, PublicKey,
    };

    use super::*;
    use crate::mint::melt::melt_saga::MeltSaga;
    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

    async fn pending_melt() -> (Mint, MeltQuoteResolution, Vec<PublicKey>) {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(2000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let invoice =
            cdk_fake_wallet::create_fake_invoice(900_000, "operator resolution".to_owned());
        let quote = mint
            .get_melt_quote(crate::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .unwrap();
        let quote_id = quote.quote().unwrap().clone();
        let quote = mint
            .localstore
            .get_melt_quote(&quote_id)
            .await
            .unwrap()
            .unwrap();
        let keyset_id = *mint.get_active_keysets().get(&CurrencyUnit::Sat).unwrap();
        let change = PreMintSecrets::blank(keyset_id, Amount::from(2000)).unwrap();
        let melt_request =
            MeltRequest::new(quote_id.clone(), proofs, Some(change.blinded_messages()));
        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let setup = MeltSaga::new(
            Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        )
        .setup_melt(
            &melt_request,
            verification,
            PaymentMethod::Known(KnownMethod::Bolt11),
        )
        .await
        .unwrap();
        drop(setup);
        let saga = mint
            .localstore
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .unwrap()
            .unwrap();
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut acquired = tx
            .get_saga_for_update(&saga.operation_id)
            .await
            .unwrap()
            .unwrap();
        tx.update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::PaymentPending),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let request = MeltQuoteResolution {
            quote_id,
            operation_id: saga.operation_id,
            expected_saga_state: MeltSagaState::PaymentPending,
            reason: "Verified backend settlement".to_owned(),
            action: MeltQuoteResolutionAction::Finalize(MeltFinalizationData {
                total_spent: Amount::new(925, CurrencyUnit::Sat),
                payment_lookup_id: quote.request_lookup_id.unwrap(),
                payment_proof: Some("operator-supplied-proof".to_owned()),
                bolt12_payer_proof_inputs: None,
            }),
        };
        (mint, request, input_ys)
    }

    async fn assert_pending(mint: &Mint, request: &MeltQuoteResolution, inputs: &[PublicKey]) {
        assert_eq!(
            mint.localstore
                .get_melt_quote(&request.quote_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            MeltQuoteState::Pending
        );
        assert!(mint
            .localstore
            .get_proofs_states(inputs)
            .await
            .unwrap()
            .iter()
            .all(|state| *state == Some(State::Pending)));
        assert!(mint
            .localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                RESOLUTION_NAMESPACE,
                &request.operation_id.to_string()
            )
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn finalize_spends_proofs_signs_change_and_replays_without_duplicating() {
        let (mint, request, inputs) = pending_melt().await;
        let result = mint.resolve_melt_quote(request.clone()).await.unwrap();
        assert_eq!(result.state, MeltQuoteState::Paid);
        assert_eq!(
            result.payment_proof.as_deref(),
            Some("operator-supplied-proof")
        );
        assert!(mint
            .localstore
            .get_proofs_states(&inputs)
            .await
            .unwrap()
            .iter()
            .all(|state| *state == Some(State::Spent)));
        let signatures = mint
            .localstore
            .get_blind_signatures_for_quote(&request.quote_id)
            .await
            .unwrap();
        assert_eq!(
            signatures
                .iter()
                .map(|sig| sig.amount.to_u64())
                .sum::<u64>(),
            1075
        );
        assert!(mint
            .localstore
            .get_completed_operation(&request.operation_id)
            .await
            .unwrap()
            .is_some());
        assert!(mint
            .localstore
            .get_melt_saga_by_quote_id(&request.quote_id)
            .await
            .unwrap()
            .is_none());
        let bytes = mint
            .localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                RESOLUTION_NAMESPACE,
                &request.operation_id.to_string(),
            )
            .await
            .unwrap()
            .unwrap();
        let record: ResolutionRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(record.request, request);
        assert!(record.accepted_at > 0);
        assert_eq!(
            mint.resolve_melt_quote(request.clone())
                .await
                .unwrap()
                .state,
            MeltQuoteState::Paid
        );
        assert_eq!(
            mint.localstore
                .get_blind_signatures_for_quote(&request.quote_id)
                .await
                .unwrap(),
            signatures
        );
        let mut conflicting = request;
        conflicting.action = MeltQuoteResolutionAction::Compensate {
            payment_failure_confirmed: true,
        };
        assert!(matches!(
            mint.resolve_melt_quote(conflicting).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn compensation_releases_proofs_and_rejects_a_later_operation() {
        let (mint, mut request, inputs) = pending_melt().await;
        request.action = MeltQuoteResolutionAction::Compensate {
            payment_failure_confirmed: true,
        };
        assert_eq!(
            mint.resolve_melt_quote(request.clone())
                .await
                .unwrap()
                .state,
            MeltQuoteState::Unpaid
        );
        assert!(mint
            .localstore
            .get_proofs_states(&inputs)
            .await
            .unwrap()
            .iter()
            .all(Option::is_none));
        assert!(mint
            .localstore
            .get_melt_saga_by_quote_id(&request.quote_id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            mint.resolve_melt_quote(request.clone())
                .await
                .unwrap()
                .state,
            MeltQuoteState::Unpaid
        );
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        assert!(tx
            .get_melt_request_and_blinded_messages(&request.quote_id)
            .await
            .unwrap()
            .is_none());
        let retained = Saga::new_melt(
            request.operation_id,
            MeltSagaState::PaymentFailed,
            request.quote_id.to_string(),
        );
        tx.add_saga(&retained).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            mint.resolve_melt_quote(request.clone())
                .await
                .unwrap()
                .state,
            MeltQuoteState::Unpaid
        );
        assert!(mint
            .localstore
            .get_melt_saga_by_quote_id(&request.quote_id)
            .await
            .unwrap()
            .is_none());
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let replacement = Saga::new_melt(
            Uuid::new_v4(),
            MeltSagaState::SetupComplete,
            request.quote_id.to_string(),
        );
        tx.add_saga(&replacement).await.unwrap();
        tx.commit().await.unwrap();
        assert!(matches!(
            mint.resolve_melt_quote(request).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        assert_eq!(
            mint.localstore
                .get_melt_saga_by_quote_id(&replacement.quote_id.unwrap().parse().unwrap())
                .await
                .unwrap()
                .unwrap()
                .operation_id,
            replacement.operation_id
        );
    }

    #[tokio::test]
    async fn invalid_amounts_stale_inspection_and_unconfirmed_failure_do_not_mutate() {
        let (mint, request, inputs) = pending_melt().await;
        let mut invalid = request.clone();
        invalid.expected_saga_state = MeltSagaState::SetupComplete;
        assert!(matches!(
            mint.resolve_melt_quote(invalid).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        let mut invalid = request.clone();
        invalid.operation_id = Uuid::new_v4();
        assert!(matches!(
            mint.resolve_melt_quote(invalid).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        let mut invalid = request.clone();
        invalid.action = MeltQuoteResolutionAction::Compensate {
            payment_failure_confirmed: false,
        };
        assert!(matches!(
            mint.resolve_melt_quote(invalid).await,
            Err(MeltQuoteResolutionError::InvalidRequest(_))
        ));
        for (value, unit) in [
            (899, CurrencyUnit::Sat),
            (2001, CurrencyUnit::Sat),
            (925, CurrencyUnit::Msat),
        ] {
            let mut invalid = request.clone();
            if let MeltQuoteResolutionAction::Finalize(payment) = &mut invalid.action {
                payment.total_spent = Amount::new(value, unit);
            }
            assert!(matches!(
                mint.resolve_melt_quote(invalid).await,
                Err(MeltQuoteResolutionError::InvalidRequest(_))
            ));
        }
        let lock = mint.melt_quote_lock(&request.quote_id).await;
        let guard = lock.lock_owned().await;
        assert!(matches!(
            mint.resolve_melt_quote(request.clone()).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        drop(guard);
        assert_pending(&mint, &request, &inputs).await;
    }

    #[tokio::test]
    async fn recorded_handoffs_resume_and_finalizing_cannot_be_compensated() {
        for (compensate, recover_on_startup) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let (mint, mut request, inputs) = pending_melt().await;
            if compensate {
                request.action = MeltQuoteResolutionAction::Compensate {
                    payment_failure_confirmed: true,
                };
            }
            // Simulate interruption after the audit and handoff commit, before cleanup.
            let mut tx = mint.localstore.begin_transaction().await.unwrap();
            let mut saga = tx
                .get_saga_for_update(&request.operation_id)
                .await
                .unwrap()
                .unwrap();
            match &request.action {
                MeltQuoteResolutionAction::Finalize(payment) => tx
                    .update_acquired_saga_with_finalization_data(
                        &mut saga,
                        SagaStateEnum::Melt(MeltSagaState::Finalizing),
                        Some(payment),
                    )
                    .await
                    .unwrap(),
                MeltQuoteResolutionAction::Compensate { .. } => tx
                    .update_acquired_saga(
                        &mut saga,
                        SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
                    )
                    .await
                    .unwrap(),
            }
            let record = ResolutionRecord {
                request: request.clone(),
                accepted_at: unix_time(),
            };
            tx.kv_write(
                CDK_MINT_PRIMARY_NAMESPACE,
                RESOLUTION_NAMESPACE,
                &request.operation_id.to_string(),
                &serde_json::to_vec(&record).unwrap(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            let mut contrary = request.clone();
            contrary.action = match compensate {
                true => MeltQuoteResolutionAction::Finalize(MeltFinalizationData {
                    total_spent: Amount::new(925, CurrencyUnit::Sat),
                    payment_lookup_id: cdk_common::payment::PaymentIdentifier::QuoteId(
                        request.quote_id.clone(),
                    ),
                    payment_proof: None,
                    bolt12_payer_proof_inputs: None,
                }),
                false => MeltQuoteResolutionAction::Compensate {
                    payment_failure_confirmed: true,
                },
            };
            assert!(matches!(
                mint.resolve_melt_quote(contrary).await,
                Err(MeltQuoteResolutionError::Conflict(_))
            ));
            if recover_on_startup {
                mint.recover_from_incomplete_melt_sagas().await.unwrap();
                assert!(mint
                    .localstore
                    .get_melt_saga_by_quote_id(&request.quote_id)
                    .await
                    .unwrap()
                    .is_none());
            }
            let expected = match compensate {
                true => MeltQuoteState::Unpaid,
                false => MeltQuoteState::Paid,
            };
            assert_eq!(
                mint.resolve_melt_quote(request.clone())
                    .await
                    .unwrap()
                    .state,
                expected
            );
            let state = match compensate {
                true => None,
                false => Some(State::Spent),
            };
            assert!(mint
                .localstore
                .get_proofs_states(&inputs)
                .await
                .unwrap()
                .iter()
                .all(|actual| *actual == state));
            assert!(mint
                .localstore
                .get_melt_saga_by_quote_id(&request.quote_id)
                .await
                .unwrap()
                .is_none());
        }
    }

    #[tokio::test]
    async fn existing_finalizing_result_is_immutable_and_can_finish_cleanup() {
        let (mint, mut request, inputs) = pending_melt().await;
        let payment = match &request.action {
            MeltQuoteResolutionAction::Finalize(payment) => payment.clone(),
            _ => unreachable!(),
        };
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&request.operation_id)
            .await
            .unwrap()
            .unwrap();
        tx.update_acquired_saga_with_finalization_data(
            &mut saga,
            SagaStateEnum::Melt(MeltSagaState::Finalizing),
            Some(&payment),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        request.expected_saga_state = MeltSagaState::Finalizing;
        let mut compensation = request.clone();
        compensation.action = MeltQuoteResolutionAction::Compensate {
            payment_failure_confirmed: true,
        };
        assert!(matches!(
            mint.resolve_melt_quote(compensation).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        let mut conflicting = request.clone();
        if let MeltQuoteResolutionAction::Finalize(payment) = &mut conflicting.action {
            payment.total_spent = Amount::new(926, CurrencyUnit::Sat);
        }
        assert!(matches!(
            mint.resolve_melt_quote(conflicting).await,
            Err(MeltQuoteResolutionError::Conflict(_))
        ));
        assert_pending(&mint, &request, &inputs).await;
        assert_eq!(
            mint.resolve_melt_quote(request).await.unwrap().state,
            MeltQuoteState::Paid
        );
    }
}
