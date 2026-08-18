//! Shared saga recovery logic for melt operations.
//!
//! This module contains functions used by both startup recovery and on-demand quote checking
//! to process melt saga outcomes consistently.

use cdk_common::mint::{MeltFinalizationData, MeltQuote, MeltSagaState, Saga, SagaStateEnum};
use cdk_common::nuts::MeltQuoteState;
use cdk_common::payment::MakePaymentResponse;
use tracing::instrument;

use crate::mint::subscription::PubSubManager;
use crate::mint::Mint;
use crate::Error;

/// Process the outcome of a melt saga based on payment status.
///
/// This function handles the shared logic for deciding whether to finalize, compensate, or skip
/// a melt operation based on the payment response from the payment backend.
///
/// For the `Paid` case, this delegates to [`super::melt::shared::finalize_melt_quote`] which
/// is the single finalization path — it handles operation recording, saga deletion, and all
/// cleanup atomically.
///
/// For the `Unpaid`/`Failed` case, compensation is only allowed after these
/// cross-replica checks:
/// 1. the melt did not settle internally (a credited mint quote can never be
///    compensated — that would return the payer's proofs while the recipient
///    keeps the credit),
/// 2. the quote-scoped advisory lock proves no replica is still dispatching,
/// 3. a fresh backend status check confirms the payment did not succeed (the
///    caller's observation may be stale),
/// 4. a payment with an ambiguous dispatch state is only compensated after a
///    trusted failure event (public `Unpaid` and `Failed` polls remain
///    fail-closed), and
/// 5. the saga has not advanced to `Finalizing`.
///
/// Any check that cannot be answered fails closed: the quote is left pending
/// rather than compensated.
///
/// # Arguments
/// * `saga` - The melt saga being processed
/// * `quote` - The melt quote associated with the saga
/// * `payment_response` - The payment status from the payment backend
/// * `db` - Database handle
/// * `pubsub` - PubSub manager for notifications
/// * `mint` - Mint instance for signing operations
///
/// # Returns
/// Ok(()) on success, or an error if processing fails
#[instrument(skip_all)]
pub(crate) async fn process_melt_saga_outcome(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    process_melt_saga_outcome_inner(saga, quote, payment_response, db, pubsub, mint, false).await
}

/// Process a failure delivered by the trusted payment-backend event stream.
/// Unlike public polling, a definitive failure event may resolve a previously
/// acknowledged pending payment after a fresh terminal backend check.
pub(crate) async fn process_melt_saga_failure_event(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    process_melt_saga_outcome_inner(saga, quote, payment_response, db, pubsub, mint, true).await
}

#[allow(clippy::too_many_arguments)]
async fn process_melt_saga_outcome_inner(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
    definitive_failure_event: bool,
) -> Result<(), Error> {
    match payment_response.status {
        MeltQuoteState::Paid => {
            // Persist the paid handoff before releasing an acquired dispatch
            // lock. When contended, the live owner holds the saga row through
            // dispatch, so finalization serializes on that row after its own
            // pending or paid handoff commits.
            match super::melt::shared::try_acquire_melt_dispatch_lock(db, &quote.id).await? {
                super::melt::shared::DispatchLockAttempt::Acquired(mut tx) => {
                    match tx.get_saga_for_update(&saga.operation_id).await? {
                        Some(mut current_saga) => {
                            if let Err(err) =
                                super::melt::shared::persist_melt_finalization_handoff(
                                    &mut tx,
                                    &mut current_saga,
                                    payment_response,
                                )
                                .await
                            {
                                tx.rollback().await?;
                                return Err(err);
                            }
                            tx.commit().await?;
                        }
                        None => {
                            tx.rollback().await?;
                        }
                    }
                }
                super::melt::shared::DispatchLockAttempt::Contended => {
                    tracing::info!(
                        "Melt quote {} dispatch lock is contended; finalizing paid outcome after the live owner releases it (saga {})",
                        quote.id,
                        saga.operation_id
                    );
                }
                super::melt::shared::DispatchLockAttempt::Unsupported => {}
            }
            finalize_paid_melt_outcome(saga, quote, payment_response, db, pubsub, mint).await
        }
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
            reconcile_terminal_melt(saga, quote, db, pubsub, mint, definitive_failure_event).await
        }
        MeltQuoteState::Pending => {
            persist_pending_after_dispatch(saga, quote, payment_response, db).await?;
            tracing::debug!(
                "Melt quote {} (saga {}) payment remains Pending",
                quote.id,
                saga.operation_id
            );
            Ok(())
        }
        MeltQuoteState::Unknown => {
            tracing::debug!(
                "Melt quote {} (saga {}) payment status still {}, skipping action",
                quote.id,
                saga.operation_id,
                payment_response.status
            );
            Ok(())
        }
    }
}

async fn persist_pending_after_dispatch(
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
) -> Result<(), Error> {
    match super::melt::shared::try_acquire_melt_dispatch_lock(db, &quote.id).await? {
        super::melt::shared::DispatchLockAttempt::Acquired(mut tx) => {
            let Some(saga) = tx.get_saga(&stale_saga.operation_id).await? else {
                tx.rollback().await?;
                *quote = db
                    .get_melt_quote(&quote.id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
                return Ok(());
            };
            *quote = tx
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?
                .inner();
            if quote.state == MeltQuoteState::Pending {
                persist_payment_lookup_id_in_transaction(&mut tx, &saga, quote, payment_response)
                    .await?;
                tx.commit().await?;
            } else {
                tx.rollback().await?;
            }
            Ok(())
        }
        super::melt::shared::DispatchLockAttempt::Contended => {
            // A live dispatch holds the lock and persists its own pending
            // marker; leave the quote pending.
            *quote = db
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            Ok(())
        }
        super::melt::shared::DispatchLockAttempt::Unsupported => {
            persist_payment_lookup_id(stale_saga, quote, payment_response, db).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_terminal_melt(
    saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
    definitive_failure_event: bool,
) -> Result<(), Error> {
    let result = match super::melt::shared::try_acquire_melt_dispatch_lock(db, &quote.id).await? {
        super::melt::shared::DispatchLockAttempt::Acquired(tx) => {
            reconcile_terminal_melt_with_dispatch_lock(
                tx,
                saga,
                quote,
                db,
                pubsub,
                mint,
                definitive_failure_event,
            )
            .await
        }
        super::melt::shared::DispatchLockAttempt::Contended => {
            // A live dispatch or reconciliation owns the quote; fail closed and
            // leave it pending rather than wait on a connection held across
            // network I/O.
            tracing::info!(
                "Melt quote {} dispatch lock is contended; leaving pending (saga {})",
                quote.id,
                saga.operation_id
            );
            *quote = db
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?;
            Ok(None)
        }
        super::melt::shared::DispatchLockAttempt::Unsupported => {
            reconcile_terminal_melt_without_advisory_lock(
                saga,
                quote,
                db,
                pubsub,
                mint,
                definitive_failure_event,
            )
            .await
        }
    };

    match result? {
        Some((saga, payment_response)) => {
            finalize_paid_melt_outcome(&saga, quote, &payment_response, db, pubsub, mint).await
        }
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_terminal_melt_with_dispatch_lock(
    mut tx: cdk_common::database::DynMintTransaction,
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
    definitive_failure_event: bool,
) -> Result<Option<(Saga, MakePaymentResponse)>, Error> {
    let Some(mut saga) = tx.get_saga_for_update(&stale_saga.operation_id).await? else {
        tx.rollback().await?;
        *quote = db
            .get_melt_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        return Ok(None);
    };
    *quote = tx
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?
        .inner();

    let internal_response = match Mint::internal_melt_settlement_response_tx(&mut tx, quote).await {
        Ok(internal_response) => internal_response,
        Err(err) => {
            tracing::error!(
                "Could not determine internal settlement state for melt quote {} (saga {}): {}. Leaving pending.",
                quote.id,
                saga.operation_id,
                err
            );
            tx.rollback().await?;
            return Ok(None);
        }
    };
    if let Some(internal_response) = internal_response {
        let saga = saga.inner();
        tx.rollback().await?;
        return Ok(Some((saga, internal_response)));
    }

    let fresh_response = match mint.check_melt_payment_status(quote).await {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(
                "Cannot verify payment status for melt quote {} (saga {}): {}. Leaving pending.",
                quote.id,
                saga.operation_id,
                err
            );
            tx.rollback().await?;
            return Ok(None);
        }
    };

    match fresh_response.status {
        MeltQuoteState::Paid => {
            if let Err(err) = super::melt::shared::persist_melt_finalization_handoff(
                &mut tx,
                &mut saga,
                &fresh_response,
            )
            .await
            {
                tx.rollback().await?;
                return Err(err);
            }
            let saga = saga.inner();
            tx.commit().await?;
            return Ok(Some((saga, fresh_response)));
        }
        MeltQuoteState::Pending => {
            persist_payment_lookup_id_in_transaction(&mut tx, &saga, quote, &fresh_response)
                .await?;
            tx.commit().await?;
            return Ok(None);
        }
        MeltQuoteState::Unknown => {
            tx.rollback().await?;
            return Ok(None);
        }
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
    }

    if matches!(
        &saga.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted | MeltSagaState::PaymentPending)
    ) && !definitive_failure_event
    {
        tracing::warn!(
            "Ignoring contradictory {} status for melt quote {} because saga {} has an ambiguous dispatch state",
            fresh_response.status,
            quote.id,
            saga.operation_id
        );
        tx.rollback().await?;
        return Ok(None);
    }

    let input_ys = tx.get_proof_ys_by_operation_id(&saga.operation_id).await?;
    let blinded_secrets = tx
        .get_melt_request_and_blinded_messages(&quote.id)
        .await?
        .map(|request| {
            request
                .change_outputs
                .into_iter()
                .map(|output| output.blinded_secret)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match super::melt::shared::rollback_melt_quote_with_dispatch_lock(
        tx,
        pubsub,
        &quote.id,
        &input_ys,
        &blinded_secrets,
        &saga.operation_id,
    )
    .await
    {
        Ok(()) => {
            *quote = db
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?;
        }
        Err(Error::UnknownPaymentState) => {}
        Err(err) => return Err(err),
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
async fn reconcile_terminal_melt_without_advisory_lock(
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
    definitive_failure_event: bool,
) -> Result<Option<(Saga, MakePaymentResponse)>, Error> {
    let current_saga = db.get_melt_saga_by_quote_id(&quote.id).await?;
    let Some(saga) = current_saga else {
        *quote = db
            .get_melt_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.state != MeltQuoteState::Paid {
            tracing::warn!(
                "Melt saga {} disappeared while quote {} is {}; leaving it unchanged",
                stale_saga.operation_id,
                quote.id,
                quote.state
            );
        }
        return Ok(None);
    };

    *quote = db
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?;

    match mint.internal_melt_settlement_response(quote).await {
        Ok(Some(internal_response)) => {
            return Ok(Some((saga, internal_response)));
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(
                "Could not determine internal settlement state for melt quote {} (saga {}): {}. Leaving pending.",
                quote.id,
                saga.operation_id,
                err
            );
            return Ok(None);
        }
    }

    let fresh_response = match mint.check_melt_payment_status(quote).await {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(
                "Cannot verify payment status for melt quote {} (saga {}): {}. Leaving pending.",
                quote.id,
                saga.operation_id,
                err
            );
            return Ok(None);
        }
    };

    match fresh_response.status {
        MeltQuoteState::Paid => return Ok(Some((saga, fresh_response))),
        MeltQuoteState::Pending => {
            persist_payment_lookup_id(&saga, quote, &fresh_response, db).await?;
            return Ok(None);
        }
        MeltQuoteState::Unknown => return Ok(None),
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
    }

    if matches!(
        &saga.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted | MeltSagaState::PaymentPending)
    ) && !definitive_failure_event
    {
        tracing::warn!(
            "Ignoring contradictory {} status for melt quote {} because saga {} has an ambiguous dispatch state",
            fresh_response.status,
            quote.id,
            saga.operation_id
        );
        return Ok(None);
    }

    let input_ys = db.get_proof_ys_by_operation_id(&saga.operation_id).await?;
    let blinded_secrets = db
        .get_blinded_secrets_by_operation_id(&saga.operation_id)
        .await?;
    let rollback = super::melt::shared::rollback_melt_quote(
        db,
        pubsub,
        &quote.id,
        &input_ys,
        &blinded_secrets,
        &saga.operation_id,
    )
    .await;

    match rollback {
        Ok(()) => {
            *quote = db
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?;
        }
        Err(Error::UnknownPaymentState) => {
            tracing::info!(
                "Rollback refused for melt quote {}; finalization owns terminality (saga {})",
                quote.id,
                saga.operation_id
            );
        }
        Err(err) => return Err(err),
    }

    Ok(None)
}

/// Persists a backend-provided lookup id so later checks can recover a pending
/// payment even when no identifier was available at quote creation.
async fn persist_payment_lookup_id(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    persist_payment_lookup_id_in_transaction(&mut tx, saga, quote, payment_response).await?;
    tx.commit().await?;
    Ok(())
}

async fn persist_payment_lookup_id_in_transaction(
    tx: &mut cdk_common::database::DynMintTransaction,
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
) -> Result<(), Error> {
    let mut current_saga = tx
        .get_saga_for_update(&saga.operation_id)
        .await?
        .ok_or(Error::Internal)?;
    if matches!(
        &current_saga.state,
        SagaStateEnum::Melt(MeltSagaState::Finalizing)
    ) {
        tracing::info!(
            "Ignoring pending payment result for melt quote {} because saga {} is already finalizing",
            quote.id,
            saga.operation_id
        );
        return Ok(());
    }

    let mut current_quote = tx
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?;
    if current_quote.request_lookup_id.as_ref() != Some(&payment_response.payment_lookup_id) {
        tx.update_melt_quote_request_lookup_id(
            &mut current_quote,
            &payment_response.payment_lookup_id,
        )
        .await?;
    }

    tx.update_acquired_saga(
        &mut current_saga,
        SagaStateEnum::Melt(MeltSagaState::PaymentPending),
    )
    .await?;

    quote.request_lookup_id = Some(payment_response.payment_lookup_id.clone());
    Ok(())
}

/// Persists the paid outcome as a durable `Finalizing` handoff, then runs the
/// single shared finalization path.
async fn finalize_paid_melt_outcome(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    tracing::info!(
        "Finalizing paid melt quote {} (saga {})",
        quote.id,
        saga.operation_id
    );

    // Persist the quote-unit payment result before finalizing so recovery uses
    // the same durable Finalizing handoff as the in-process finalize path.
    let total_spent =
        super::melt::shared::total_spent_for_quote_unit(&payment_response.total_spent, &quote.unit)
            .map_err(|e| {
                tracing::error!(
                    "Failed to convert recovered total_spent for quote {}: {:?}",
                    quote.id,
                    e
                );
                Error::UnitMismatch
            })?;

    let mut tx = db.begin_transaction().await?;

    // The saga row is the recovery record for this handoff. If it is already
    // gone, either finalization completed earlier (idempotent re-delivery of a
    // paid outcome) or a concurrent path removed the record; only the former
    // may succeed silently.
    let Some(mut acquired_saga) = tx.get_saga_for_update(&saga.operation_id).await? else {
        tx.rollback().await?;
        return match db.get_melt_quote(&quote.id).await? {
            Some(current) if current.state == MeltQuoteState::Paid => {
                tracing::info!(
                    "Melt quote {} already finalized; ignoring paid outcome for missing saga {}",
                    quote.id,
                    saga.operation_id
                );
                *quote = current;
                Ok(())
            }
            _ => {
                tracing::error!(
                    "Paid outcome for melt quote {} but saga {} is missing and quote is not Paid",
                    quote.id,
                    saga.operation_id
                );
                Err(Error::Internal)
            }
        };
    };

    let finalization_data = MeltFinalizationData {
        total_spent: total_spent.clone(),
        payment_lookup_id: payment_response.payment_lookup_id.clone(),
        payment_proof: payment_response.payment_proof.clone(),
    };
    tx.update_acquired_saga_with_finalization_data(
        &mut acquired_saga,
        SagaStateEnum::Melt(MeltSagaState::Finalizing),
        Some(&finalization_data),
    )
    .await?;
    tx.commit().await?;

    // finalize_melt_quote handles the rest of the atomic cleanup:
    // operation recording, saga deletion, and melt request cleanup.
    super::melt::shared::finalize_melt_quote(
        mint,
        db,
        pubsub,
        quote,
        total_spent,
        payment_response.payment_proof.clone(),
        &payment_response.payment_lookup_id,
        Some(saga.operation_id),
    )
    .await?;

    // Reflect the finalized state in the caller's in-memory copy so responses
    // built from it are accurate.
    quote.state = MeltQuoteState::Paid;
    quote.payment_proof = payment_response.payment_proof.clone();
    quote.request_lookup_id = Some(payment_response.payment_lookup_id.clone());

    Ok(())
}

#[cfg(test)]
mod tests {
    use cdk_common::mint::{OperationKind, Saga};
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{CurrencyUnit, MeltQuoteBolt11Request, ProofsMethods, State};
    use cdk_common::payment::PaymentIdentifier;
    use cdk_common::{Amount, PaymentMethod};
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};

    use super::*;
    use crate::mint::melt::melt_saga::MeltSaga;
    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

    #[tokio::test]
    async fn test_paid_outcome_finalizes_and_records_completed_operation() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let (payment_saga, decision) = setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap();
        let _confirmed_saga = payment_saga.make_payment(decision).await.unwrap();

        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("paid_outcome_lookup".to_string()),
            payment_proof: Some("paid_outcome_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Spent)).await;

        let completed_operation = mint
            .localstore
            .get_completed_operation(&operation_id)
            .await
            .unwrap()
            .expect("completed operation should be recorded");
        assert_eq!(completed_operation.kind(), OperationKind::Melt);
        assert_eq!(completed_operation.id(), &operation_id);

        let paid_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should exist after finalization");
        assert_eq!(paid_quote.state, MeltQuoteState::Paid);
    }

    #[tokio::test]
    async fn test_failed_outcome_rolls_back_and_deletes_saga() {
        // Compensation requires the fresh backend re-check to confirm the
        // failure, so seed the fake backend with a terminal Failed state for
        // this invoice's payment hash.
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let amount_msats: u64 = Amount::from(9_000).into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );
        let payment_states = std::collections::HashMap::from([(
            invoice.payment_hash().to_string(),
            (
                MeltQuoteState::Failed,
                Amount::from(9_000).with_unit(CurrencyUnit::Sat),
            ),
        )]);
        let mint = create_test_mint_with_payment_states(payment_states)
            .await
            .unwrap();

        let request = cdk_common::melt::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let quote_response = mint.get_melt_quote(request).await.unwrap();
        let quote = mint
            .localstore
            .get_melt_quote(quote_response.quote().unwrap())
            .await
            .unwrap()
            .expect("quote should exist in database");

        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup_saga);

        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("failed_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, None).await;

        let recovered_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist after rollback");
        assert_eq!(recovered_quote.state, MeltQuoteState::Unpaid);
    }

    #[tokio::test]
    async fn test_ambiguous_dispatch_recovers_only_from_definitive_failure() {
        for saga_state in [
            MeltSagaState::PaymentAttempted,
            MeltSagaState::PaymentPending,
        ] {
            for fresh_status in [MeltQuoteState::Unpaid, MeltQuoteState::Failed] {
                let fake_description = FakeInvoiceDescription {
                    pay_invoice_state: fresh_status,
                    check_payment_state: fresh_status,
                    pay_err: false,
                    check_err: false,
                };
                let amount_msats: u64 = Amount::from(9_000).into();
                let invoice = create_fake_invoice(
                    amount_msats,
                    serde_json::to_string(&fake_description).unwrap(),
                );
                let payment_states = std::collections::HashMap::from([(
                    invoice.payment_hash().to_string(),
                    (
                        fresh_status,
                        Amount::from(9_000).with_unit(CurrencyUnit::Sat),
                    ),
                )]);
                let mint = create_test_mint_with_payment_states(payment_states)
                    .await
                    .unwrap();
                let request = cdk_common::melt::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                    request: invoice,
                    unit: CurrencyUnit::Sat,
                    options: None,
                });
                let quote_response = mint.get_melt_quote(request).await.unwrap();
                let quote = mint
                    .localstore
                    .get_melt_quote(quote_response.quote().unwrap())
                    .await
                    .unwrap()
                    .expect("quote should exist");
                let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
                let input_ys = proofs.ys().unwrap();
                let melt_request = create_test_melt_request(&proofs, &quote);
                let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
                let saga = MeltSaga::new(
                    std::sync::Arc::new(mint.clone()),
                    mint.localstore(),
                    mint.pubsub_manager(),
                );
                let setup_saga = saga
                    .setup_melt(
                        &melt_request,
                        verification,
                        PaymentMethod::Known(KnownMethod::Bolt11),
                    )
                    .await
                    .unwrap();
                let operation_id = assert_single_melt_saga_operation_id(&mint).await;
                drop(setup_saga);

                let mut tx = mint.localstore.begin_transaction().await.unwrap();
                let mut acquired_saga = tx
                    .get_saga_for_update(&operation_id)
                    .await
                    .unwrap()
                    .expect("saga should exist");
                tx.update_acquired_saga(
                    &mut acquired_saga,
                    SagaStateEnum::Melt(saga_state.clone()),
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();

                let saga = assert_saga_exists(&mint, &operation_id).await;
                let mut quote = mint
                    .localstore
                    .get_melt_quote(&quote.id)
                    .await
                    .unwrap()
                    .expect("quote should exist");
                let payment_response = MakePaymentResponse {
                    payment_lookup_id: quote
                        .request_lookup_id
                        .clone()
                        .expect("bolt11 quote should have a lookup id"),
                    payment_proof: None,
                    status: fresh_status,
                    total_spent: quote.amount(),
                };

                process_melt_saga_outcome(
                    &saga,
                    &mut quote,
                    &payment_response,
                    &mint.localstore,
                    &mint.pubsub_manager,
                    &mint,
                )
                .await
                .unwrap();

                let _ = assert_saga_exists(&mint, &operation_id).await;
                assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;
                assert_eq!(quote.state, MeltQuoteState::Pending);

                let stored_saga = assert_saga_exists(&mint, &operation_id).await;
                assert_eq!(stored_saga.state, SagaStateEnum::Melt(saga_state.clone()));

                process_melt_saga_failure_event(
                    &stored_saga,
                    &mut quote,
                    &payment_response,
                    &mint.localstore,
                    &mint.pubsub_manager,
                    &mint,
                )
                .await
                .unwrap();

                assert_saga_not_exists(&mint, &operation_id).await;
                assert_proofs_state(&mint, &input_ys, None).await;
                assert_eq!(quote.state, MeltQuoteState::Unpaid);
            }
        }
    }

    #[tokio::test]
    async fn test_failed_outcome_for_already_paid_quote_is_no_op() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let (payment_saga, decision) = setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap();
        let confirmed_saga = match payment_saga.make_payment(decision).await.unwrap() {
            crate::mint::melt::melt_saga::PaymentOutcome::Confirmed(confirmed_saga) => {
                confirmed_saga
            }
            crate::mint::melt::melt_saga::PaymentOutcome::Pending { .. } => {
                panic!("Expected confirmed payment outcome")
            }
        };

        confirmed_saga.finalize().await.unwrap();

        let mut paid_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();

        let saga = Saga {
            operation_id,
            operation_kind: OperationKind::Melt,
            quote_id: Some(quote.id.to_string()),
            state: SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
            created_at: 0,
            finalization_data: None,
            updated_at: 0,
        };
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("failed_after_paid_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: paid_quote.amount(),
        };

        // The stale failure must not roll back the paid quote: the fresh
        // backend check reports Paid, and the already-completed finalization
        // (saga deleted, quote Paid) makes the paid outcome an idempotent
        // no-op instead of an error.
        process_melt_saga_outcome(
            &saga,
            &mut paid_quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_eq!(paid_quote.state, MeltQuoteState::Paid);

        let persisted_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(persisted_quote.state, MeltQuoteState::Paid);
    }

    /// A successful payment event may finish finalization after a status check
    /// loaded a stale saga and quote. If the backend still reports `Failed`,
    /// the stale path reaches rollback after the finalizer deleted the saga.
    /// The rollback no-op must refresh the caller's quote from the database
    /// instead of reporting `Unpaid` for the persisted paid melt.
    #[tokio::test]
    async fn test_stale_failed_outcome_reloads_paid_quote_after_finalization() {
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let amount_msats: u64 = Amount::from(9_000).into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );
        let payment_states = std::collections::HashMap::from([(
            invoice.payment_hash().to_string(),
            (
                MeltQuoteState::Failed,
                Amount::from(9_000).with_unit(CurrencyUnit::Sat),
            ),
        )]);
        let mint = create_test_mint_with_payment_states(payment_states)
            .await
            .unwrap();

        let request = cdk_common::melt::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let quote_response = mint.get_melt_quote(request).await.unwrap();
        let quote_id = quote_response.quote().unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let melt_quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .expect("quote should exist");
        let melt_request = create_test_melt_request(&proofs, &melt_quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();
        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup_saga);

        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga(
            &mut saga,
            SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let stale_saga = assert_saga_exists(&mint, &operation_id).await;
        let mut stale_quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .expect("quote should exist");
        let mut finalizer_quote = stale_quote.clone();
        let paid_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("paid_event_lookup".to_string()),
            payment_proof: Some("paid_event_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
        };

        finalize_paid_melt_outcome(
            &stale_saga,
            &mut finalizer_quote,
            &paid_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        let failed_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("stale_failed_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: stale_quote.amount(),
        };
        process_melt_saga_outcome(
            &stale_saga,
            &mut stale_quote,
            &failed_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_eq!(stale_quote.state, MeltQuoteState::Paid);
        assert_eq!(
            stale_quote.payment_proof.as_deref(),
            Some("paid_event_preimage")
        );
        assert_saga_not_exists(&mint, &operation_id).await;

        let persisted_quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(persisted_quote.state, MeltQuoteState::Paid);
    }

    #[tokio::test]
    async fn test_failed_outcome_for_internal_settlement_finalizes_not_rolls_back() {
        use std::str::FromStr;

        use cdk_common::nuts::MintQuoteState;
        use cdk_common::{MintQuoteBolt11Request, MintQuoteBolt11Response, QuoteId};

        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();

        // A mint quote on THIS mint. Its invoice is what makes the melt below
        // an *internal* settlement.
        let mint_quote_response: MintQuoteBolt11Response<_> = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(4_000),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .into();
        let mint_quote_id = QuoteId::from_str(&mint_quote_response.quote).unwrap();
        let mint_quote = mint
            .localstore
            .get_mint_quote(&mint_quote_id)
            .await
            .unwrap()
            .expect("mint quote should exist");

        // Melt quote whose request is the mint quote's invoice -> internal
        // settlement.
        let melt_quote_request =
            cdk_common::melt::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: mint_quote.request.to_string().parse().unwrap(),
                unit: CurrencyUnit::Sat,
                options: None,
            });
        let melt_quote_response = mint.get_melt_quote(melt_quote_request).await.unwrap();
        let melt_quote = mint
            .localstore
            .get_melt_quote(melt_quote_response.quote().expect("single-quote method"))
            .await
            .unwrap()
            .expect("melt quote should exist");

        let melt_request = create_test_melt_request(&proofs, &melt_quote);
        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();
        let operation_id = assert_single_melt_saga_operation_id(&mint).await;

        // Internal settlement commits: credits the mint quote and moves the
        // saga to PaymentAttempted, but does NOT finalize (proofs Pending).
        let (payment_saga, _decision) = setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap();

        // Simulate a crash / interleaving before finalize.
        drop(payment_saga);

        // Pre-condition: mint quote credited (Paid); payer's proofs Pending.
        let mint_quote_before = mint
            .localstore
            .get_mint_quote(&mint_quote_id)
            .await
            .unwrap()
            .expect("mint quote should exist");
        assert_eq!(mint_quote_before.state(), MintQuoteState::Paid);
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let mut quote = mint
            .localstore
            .get_melt_quote(&melt_quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;

        // On-demand recovery sees a non-paid status for the (never-attempted)
        // external payment of this internally-settled quote.
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId(
                "internal_settlement_failed_lookup".to_string(),
            ),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        // The recipient's mint quote stays credited AND the payer's proofs are
        // consumed: recovery finalized instead of compensating.
        let mint_quote_after = mint
            .localstore
            .get_mint_quote(&mint_quote_id)
            .await
            .unwrap()
            .expect("mint quote should exist");
        assert_eq!(mint_quote_after.state(), MintQuoteState::Paid);

        assert_proofs_state(&mint, &input_ys, Some(State::Spent)).await;
        assert_eq!(quote.state, MeltQuoteState::Paid);
        assert_saga_not_exists(&mint, &operation_id).await;
    }

    #[tokio::test]
    async fn test_failed_outcome_does_not_rollback_finalizing_saga() {
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let amount_msats: u64 = Amount::from(9_000).into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );
        // The fresh backend re-check confirms the failure, so compensation
        // proceeds all the way to the rollback guard.
        let payment_states = std::collections::HashMap::from([(
            invoice.payment_hash().to_string(),
            (
                MeltQuoteState::Failed,
                Amount::from(9_000).with_unit(CurrencyUnit::Sat),
            ),
        )]);
        let mint = create_test_mint_with_payment_states(payment_states)
            .await
            .unwrap();

        let request = cdk_common::melt::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });
        let quote_response = mint.get_melt_quote(request).await.unwrap();
        let quote_id = quote_response.quote().unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .unwrap();
        let melt_request = create_test_melt_request(&proofs, &quote);
        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();
        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup_saga);

        // Move the saga to PaymentAttempted and load a stale snapshot of it.
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga(
            &mut saga,
            SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let stale_saga = assert_saga_exists(&mint, &operation_id).await;

        // A concurrent finalizer commits the Finalizing handoff.
        let finalization_data = MeltFinalizationData {
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
            payment_lookup_id: PaymentIdentifier::CustomId("paid_lookup".to_string()),
            payment_proof: Some("paid_preimage".to_string()),
        };
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga_with_finalization_data(
            &mut saga,
            SagaStateEnum::Melt(MeltSagaState::Finalizing),
            Some(&finalization_data),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let mut quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .unwrap();
        let failed_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("failed_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: quote.amount(),
        };
        process_melt_saga_outcome(
            &stale_saga,
            &mut quote,
            &failed_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        // The stale failure must not undo the Finalizing melt: proofs stay
        // reserved, the quote stays pending, and the saga survives for the
        // finalizer.
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;
        let persisted_quote = mint
            .localstore
            .get_melt_quote(quote_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted_quote.state, MeltQuoteState::Pending);
        let saga_after = assert_saga_exists(&mint, &operation_id).await;
        assert_eq!(
            saga_after.state,
            SagaStateEnum::Melt(MeltSagaState::Finalizing)
        );
    }

    #[tokio::test]
    async fn test_stale_pending_outcome_does_not_regress_finalizing_saga() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let stale_saga = assert_saga_exists(&mint, &operation_id).await;
        let finalization_data = MeltFinalizationData {
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
            payment_lookup_id: PaymentIdentifier::CustomId("paid_lookup".to_string()),
            payment_proof: Some("paid_preimage".to_string()),
        };
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga_with_finalization_data(
            &mut saga,
            SagaStateEnum::Melt(MeltSagaState::Finalizing),
            Some(&finalization_data),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("pending_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &stale_saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        let persisted_saga = assert_saga_exists(&mint, &operation_id).await;
        assert_eq!(
            persisted_saga.state,
            SagaStateEnum::Melt(MeltSagaState::Finalizing)
        );
        assert_eq!(persisted_saga.finalization_data, Some(finalization_data));
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    }

    #[tokio::test]
    async fn test_unknown_outcome_leaves_state_unchanged() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("unknown_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: quote.amount(),
        };

        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        assert_saga_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(pending_quote.state, MeltQuoteState::Pending);
    }

    #[tokio::test]
    async fn test_paid_outcome_with_unit_mismatch_returns_error_without_mutation() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            std::sync::Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let saga = assert_saga_exists(&mint, &operation_id).await;
        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("unit_mismatch_lookup".to_string()),
            payment_proof: Some("unit_mismatch_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Usd),
        };

        let err = process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, Error::UnitMismatch));
        assert_saga_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;

        let still_pending_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(still_pending_quote.state, MeltQuoteState::Pending);

        let completed_operation = mint
            .localstore
            .get_completed_operation(&operation_id)
            .await
            .unwrap();
        assert!(completed_operation.is_none());
    }

    /// Creates a test mint whose fake backend reports the given pre-seeded
    /// payment states from `check_outgoing_payment` (keyed by payment hash).
    async fn create_test_mint_with_payment_states(
        payment_states: std::collections::HashMap<String, (MeltQuoteState, Amount<CurrencyUnit>)>,
    ) -> Result<crate::mint::Mint, Error> {
        use crate::mint::{MintBuilder, MintMeltLimits};
        use crate::types::{FeeReserve, QuoteTTL};

        let db = std::sync::Arc::new(cdk_sqlite::mint::memory::empty().await?);
        let mut mint_builder = MintBuilder::new(db.clone());

        let fee_reserve = FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };
        let backend = cdk_fake_wallet::FakeWallet::new(
            fee_reserve,
            payment_states,
            std::collections::HashSet::default(),
            2,
            CurrencyUnit::Sat,
        );

        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                std::sync::Arc::new(backend),
            )
            .await?;

        let mnemonic = bip39::Mnemonic::generate(12).map_err(|e| Error::Custom(e.to_string()))?;
        let mint = mint_builder
            .with_name("test mint".to_string())
            .with_description("test mint for saga recovery tests".to_string())
            .with_urls(vec!["https://test-mint".to_string()])
            .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
            .await?;

        mint.set_quote_ttl(QuoteTTL::new(10000, 10000)).await?;
        mint.start().await?;

        Ok(mint)
    }

    async fn create_test_melt_quote(mint: &crate::mint::Mint, amount: Amount) -> MeltQuote {
        use cdk_common::melt::MeltQuoteRequest;

        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };

        let amount_msats: u64 = amount.into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );

        let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });

        let quote_response = mint.get_melt_quote(request).await.unwrap();

        mint.localstore
            .get_melt_quote(quote_response.quote().unwrap())
            .await
            .unwrap()
            .expect("quote should exist in database")
    }

    fn create_test_melt_request(
        proofs: &cdk_common::nuts::Proofs,
        quote: &MeltQuote,
    ) -> cdk_common::nuts::MeltRequest<cdk_common::QuoteId> {
        cdk_common::nuts::MeltRequest::new(quote.id.clone(), proofs.clone(), None)
    }

    async fn assert_saga_exists(mint: &crate::mint::Mint, operation_id: &uuid::Uuid) -> Saga {
        mint.localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.operation_id == *operation_id)
            .expect("saga should exist in database")
    }

    async fn assert_single_melt_saga_operation_id(mint: &crate::mint::Mint) -> uuid::Uuid {
        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();

        assert_eq!(sagas.len(), 1, "expected exactly one melt saga");
        sagas[0].operation_id
    }

    async fn assert_saga_not_exists(mint: &crate::mint::Mint, operation_id: &uuid::Uuid) {
        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();

        assert!(
            !sagas.iter().any(|s| s.operation_id == *operation_id),
            "saga should not exist in database"
        );
    }

    async fn assert_proofs_state(
        mint: &crate::mint::Mint,
        ys: &[cdk_common::PublicKey],
        expected_state: Option<State>,
    ) {
        let states = mint.localstore.get_proofs_states(ys).await.unwrap();

        for state in states {
            assert_eq!(state, expected_state, "proof state mismatch");
        }
    }
}
