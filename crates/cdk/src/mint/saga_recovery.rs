//! Shared saga recovery logic for melt operations.
//!
//! This module contains functions used by both startup recovery and on-demand quote checking
//! to process melt saga outcomes consistently.

use cdk_common::mint::{MeltFinalizationData, MeltQuote, MeltSagaState, Saga, SagaStateEnum};
use cdk_common::nuts::MeltQuoteState;
use cdk_common::payment::MakePaymentResponse;
use tracing::instrument;

use crate::mint::melt::lease::{check_melt_lease, MeltLease};
use crate::mint::subscription::PubSubManager;
use crate::mint::Mint;
use crate::Error;

/// Error handling policy for backend/recovery errors during melt saga reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendErrorPolicy {
    /// Log errors and skip the saga so batch recovery continues with remaining sagas.
    LogAndSkip,
    /// Propagate errors to the caller (used by on-demand quote status checks).
    Propagate,
}

impl BackendErrorPolicy {
    fn resolve<T, F>(self, result: Result<T, Error>, log_err: F) -> Result<Option<T>, Error>
    where
        F: FnOnce(&Error),
    {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(err) => match self {
                Self::Propagate => Err(err),
                Self::LogAndSkip => {
                    log_err(&err);
                    Ok(None)
                }
            },
        }
    }

    fn handle<F>(self, result: Result<(), Error>, log_err: F) -> Result<(), Error>
    where
        F: FnOnce(&Error),
    {
        self.resolve(result, log_err).map(|_| ())
    }
}

/// Pure recovery decision for a persisted melt saga and its current quote state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MeltRecoveryStep<'a> {
    /// Setup committed before payment was attempted; compensate and release proofs.
    CompensateSetup,
    /// Authoritative payment failure was recorded; compensate and release proofs.
    CompensateRecordedFailure,
    /// Durable finalization payload is already persisted; finish cleanup directly.
    Finalize(&'a MeltFinalizationData),
    /// Legacy `Finalizing` row without payload; reconstruct payment data from
    /// internal settlement or the payment backend and finish cleanup.
    ResolveLegacyFinalizing,
    /// Payment may have been dispatched; check internal settlement and then the
    /// payment backend.
    ResolveAttemptedPayment,
    /// Current quote state does not permit action for this saga state.
    Noop,
}

/// Decides the recovery action for a persisted melt saga state and quote state.
pub(crate) fn plan_melt_recovery(
    state: &MeltSagaState,
    quote_state: MeltQuoteState,
) -> MeltRecoveryStep<'_> {
    match state {
        MeltSagaState::SetupComplete => match quote_state {
            MeltQuoteState::Pending | MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                MeltRecoveryStep::CompensateSetup
            }
            MeltQuoteState::Paid | MeltQuoteState::Unknown => MeltRecoveryStep::Noop,
        },
        MeltSagaState::PaymentFailed => MeltRecoveryStep::CompensateRecordedFailure,
        MeltSagaState::Finalizing(Some(data)) => MeltRecoveryStep::Finalize(data),
        MeltSagaState::Finalizing(None) => MeltRecoveryStep::ResolveLegacyFinalizing,
        MeltSagaState::PaymentAttempted | MeltSagaState::PaymentPending => {
            MeltRecoveryStep::ResolveAttemptedPayment
        }
    }
}

/// Reconciles an incomplete melt saga under an already-claimed [`MeltLease`].
///
/// Both startup recovery and on-demand quote checks execute this single
/// reconciliation state machine, differing only in [`BackendErrorPolicy`].
#[instrument(skip_all)]
pub(crate) async fn reconcile_melt_saga_with_lease(
    saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
    error_policy: BackendErrorPolicy,
) -> Result<(), Error> {
    let Some(melt_state) = saga.melt_state() else {
        return Ok(());
    };

    match plan_melt_recovery(melt_state, quote.state) {
        MeltRecoveryStep::CompensateSetup => {
            tracing::info!(
                "Saga {} in SetupComplete state - payment never attempted, will compensate",
                saga.operation_id
            );
            let input_ys = db.get_proof_ys_by_operation_id(&saga.operation_id).await?;
            let blinded_secrets = db
                .get_blinded_secrets_by_operation_id(&saga.operation_id)
                .await?;

            tracing::info!(
                "Compensating melt saga {} (removing {} proofs, {} change outputs)",
                saga.operation_id,
                input_ys.len(),
                blinded_secrets.len()
            );

            if error_policy
                .resolve(
                    super::melt::shared::rollback_setup_melt_quote(
                        db,
                        pubsub,
                        &quote.id,
                        &input_ys,
                        &blinded_secrets,
                        &saga.operation_id,
                        &quote.melt_lock,
                    )
                    .await,
                    |err| {
                        tracing::error!(
                            "Failed to rollback melt quote {} for saga {}: {}",
                            quote.id,
                            saga.operation_id,
                            err
                        );
                    },
                )?
                .is_some()
            {
                *quote = db
                    .get_melt_quote(&quote.id)
                    .await?
                    .ok_or(Error::UnknownQuote)?;
            }
            Ok(())
        }
        MeltRecoveryStep::CompensateRecordedFailure => {
            tracing::info!(
                "Saga {} has an authoritative payment failure - will compensate",
                saga.operation_id
            );
            error_policy.handle(
                recover_recorded_payment_failure_with_lease(saga, quote, db, pubsub).await,
                |err| {
                    tracing::error!(
                        "Failed to rollback failed melt quote {} for saga {}: {}",
                        quote.id,
                        saga.operation_id,
                        err
                    );
                },
            )
        }
        MeltRecoveryStep::Finalize(data) => {
            tracing::info!(
                "Saga {} in Finalizing state - finalizing change and cleanup",
                saga.operation_id
            );
            if error_policy
                .resolve(
                    complete_finalization_with_data(saga, quote, data, db, pubsub, mint).await,
                    |err| {
                        tracing::error!(
                            "Failed to finalize Finalizing saga {}: {}. Will retry.",
                            saga.operation_id,
                            err
                        );
                    },
                )?
                .is_some()
            {
                tracing::info!(
                    "Successfully recovered Finalizing saga {}",
                    saga.operation_id
                );
            }
            Ok(())
        }
        MeltRecoveryStep::ResolveLegacyFinalizing => {
            tracing::info!(
                "Saga {} in legacy Finalizing state without payload - resolving payment details",
                saga.operation_id
            );
            let Some(Some(payment_response)) =
                error_policy.resolve(mint.recover_finalizing_saga(saga, quote).await, |err| {
                    tracing::error!(
                        "Failed to recover legacy Finalizing saga {}: {}. Skipping.",
                        saga.operation_id,
                        err
                    );
                })?
            else {
                return Ok(());
            };

            if error_policy
                .resolve(
                    finalize_paid_melt_outcome(saga, quote, &payment_response, db, pubsub, mint)
                        .await,
                    |err| {
                        tracing::error!(
                            "Failed to finalize legacy Finalizing saga {}: {}. Will retry.",
                            saga.operation_id,
                            err
                        );
                    },
                )?
                .is_some()
            {
                tracing::info!(
                    "Successfully recovered legacy Finalizing saga {}",
                    saga.operation_id
                );
            }
            Ok(())
        }
        MeltRecoveryStep::ResolveAttemptedPayment => {
            tracing::info!(
                "Saga {} in {} state - checking for internal or external payment",
                saga.operation_id,
                melt_state
            );

            // Check internal settlement first; fail closed on DB errors.
            let Some(internal_response) = error_policy.resolve(
                mint.internal_melt_settlement_response(quote).await,
                |err| {
                    tracing::error!(
                        "Failed to determine internal settlement for saga {}: {}. Leaving pending.",
                        saga.operation_id,
                        err
                    );
                },
            )?
            else {
                return Ok(());
            };

            if let Some(payment_response) = internal_response {
                tracing::info!(
                    "Saga {} was internal settlement - will finalize directly",
                    saga.operation_id
                );
                return error_policy.handle(
                    finalize_paid_melt_outcome(saga, quote, &payment_response, db, pubsub, mint)
                        .await,
                    |err| {
                        tracing::error!(
                            "Failed to finalize internal settlement saga {}: {}. Will retry on next recovery cycle.",
                            saga.operation_id,
                            err
                        );
                    },
                );
            }

            if quote.state != MeltQuoteState::Pending {
                return Ok(());
            }

            let Some(payment_response) =
                error_policy.resolve(mint.check_melt_payment_status(quote).await, |err| {
                    tracing::warn!(
                        "Failed to check payment status for saga {} quote {}: {}. Skipping for now, will retry on next recovery cycle.",
                        saga.operation_id,
                        quote.id,
                        err
                    );
                })?
            else {
                return Ok(());
            };

            if error_policy == BackendErrorPolicy::LogAndSkip {
                match payment_response.status {
                    MeltQuoteState::Unknown => {
                        tracing::warn!(
                            saga_id = %saga.operation_id,
                            quote_id = %quote.id,
                            payment_lookup_id = %payment_response.payment_lookup_id,
                            payment_status = %payment_response.status,
                            "recovery cannot prove payment failure; quote and proofs remain pending",
                        );
                    }
                    MeltQuoteState::Pending => {
                        tracing::info!(
                            saga_id = %saga.operation_id,
                            quote_id = %quote.id,
                            payment_lookup_id = %payment_response.payment_lookup_id,
                            "recovery found an in-flight payment; quote and proofs remain pending",
                        );
                    }
                    MeltQuoteState::Paid | MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
                }
            }

            error_policy.handle(
                process_melt_saga_outcome_with_lease(
                    saga,
                    quote,
                    &payment_response,
                    db,
                    pubsub,
                    mint,
                )
                .await,
                |err| {
                    tracing::error!(
                        "Failed to process melt saga {}: {}. Will retry on next recovery cycle.",
                        saga.operation_id,
                        err
                    );
                },
            )
        }
        MeltRecoveryStep::Noop => Ok(()),
    }
}

/// Runs the shared finalization path for a saga already in `Finalizing`.
async fn complete_finalization_with_data(
    saga: &Saga,
    quote: &mut MeltQuote,
    data: &MeltFinalizationData,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    let total_spent =
        super::melt::shared::total_spent_for_quote_unit(&data.total_spent, &quote.unit).map_err(
            |e| {
                tracing::error!(
                    "Failed to convert recovered total_spent for quote {}: {:?}",
                    quote.id,
                    e
                );
                Error::UnitMismatch
            },
        )?;

    super::melt::shared::finalize_melt_quote(
        mint,
        db,
        pubsub,
        quote,
        total_spent,
        data.payment_proof.clone(),
        &data.payment_lookup_id,
        Some(saga.operation_id),
    )
    .await?;

    quote.state = MeltQuoteState::Paid;
    quote.payment_proof = data.payment_proof.clone();
    quote.request_lookup_id = Some(data.payment_lookup_id.clone());

    Ok(())
}

/// Process the outcome of a melt saga based on payment status.
///
/// This function handles the shared logic for deciding whether to finalize, compensate, or skip
/// a melt operation based on the payment response from the payment backend.
///
/// For the `Paid` case, this delegates to [`super::melt::shared::finalize_melt_quote`] which
/// is the single finalization path — it handles operation recording, saga deletion, and all
/// cleanup atomically.
///
/// Polling is finalization-only for non-terminal states: `Pending` and
/// `Unknown` observations leave the melt pending because a payment
/// orchestrator may be between attempts. `Unpaid` and `Failed` are
/// authoritative terminal outcomes by backend contract (see
/// [`cdk_common::payment::MintPayment::check_outgoing_payment`]); the failure
/// is persisted as `PaymentFailed` before compensation so a crash cannot turn
/// it back into an ambiguous payment attempt.
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
    let Some(mut lease) = MeltLease::claim(db, quote).await? else {
        return Ok(());
    };
    let result = lease
        .run(process_melt_saga_outcome_with_lease(
            saga,
            quote,
            payment_response,
            db,
            pubsub,
            mint,
        ))
        .await;
    lease.release().await;
    result
}

pub(crate) async fn process_melt_saga_outcome_with_lease(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    match payment_response.status {
        MeltQuoteState::Paid => {
            finalize_paid_melt_outcome(saga, quote, payment_response, db, pubsub, mint).await
        }
        MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
            match mint.internal_melt_settlement_response(quote).await {
                Ok(Some(internal_response)) => {
                    return finalize_paid_melt_outcome(
                        saga,
                        quote,
                        &internal_response,
                        db,
                        pubsub,
                        mint,
                    )
                    .await;
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::error!(
                        "Could not determine internal settlement state for melt quote {} (saga {}): {}. Leaving pending.",
                        quote.id,
                        saga.operation_id,
                        err
                    );
                    return Ok(());
                }
            }

            persist_permanent_payment_failure(saga, quote, db, payment_response).await?;

            recover_recorded_payment_failure_with_lease(saga, quote, db, pubsub).await
        }
        MeltQuoteState::Pending => {
            persist_pending_after_dispatch(saga, quote, payment_response, db).await?;
            tracing::trace!(
                "Melt quote {} (saga {}) payment remains Pending",
                quote.id,
                saga.operation_id
            );
            Ok(())
        }
        MeltQuoteState::Unknown => {
            // Dispatch and startup recovery report unresolved outcomes at WARN.
            // Status requests can repeat indefinitely without a state change.
            tracing::trace!(
                quote_id = %quote.id,
                saga_id = %saga.operation_id,
                payment_lookup_id = %payment_response.payment_lookup_id,
                "payment status is unknown; quote and proofs remain pending because payment failure is not authoritative",
            );
            Ok(())
        }
    }
}

/// Process a permanent failure delivered by the payment-backend event stream.
///
/// [`cdk_common::payment::Event::PaymentFailed`] is an orchestration-level
/// terminal signal, so event and polling outcomes share the same processing
/// path.
#[cfg(test)]
pub(crate) async fn process_melt_saga_failure_event(
    saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
    mint: &Mint,
) -> Result<(), Error> {
    process_melt_saga_outcome(saga, quote, payment_response, db, pubsub, mint).await
}

async fn persist_pending_after_dispatch(
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    payment_response: &MakePaymentResponse,
    db: &cdk_common::database::DynMintDatabase,
) -> Result<(), Error> {
    persist_payment_lookup_id(stale_saga, quote, payment_response, db).await
}

async fn persist_permanent_payment_failure(
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    payment_response: &MakePaymentResponse,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    let owned = tx
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?;
    check_melt_lease(&mut tx, &owned, &quote.melt_lock).await?;
    let Some(mut current_saga) = tx.get_saga_for_update(&stale_saga.operation_id).await? else {
        tx.rollback().await?;
        return Ok(());
    };

    match &current_saga.state {
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted | MeltSagaState::PaymentPending) => {}
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed) => {
            tx.rollback().await?;
            return Ok(());
        }
        SagaStateEnum::Melt(MeltSagaState::SetupComplete | MeltSagaState::Finalizing(_))
        | SagaStateEnum::Swap(_) => {
            tracing::warn!(
                "Ignoring permanent payment failure for melt quote {} because saga {} is {}",
                quote.id,
                stale_saga.operation_id,
                current_saga.state.state()
            );
            tx.rollback().await?;
            return Ok(());
        }
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
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
    )
    .await?;
    tx.commit().await?;
    quote.request_lookup_id = Some(payment_response.payment_lookup_id.clone());
    Ok(())
}

/// Compensate a melt only when a durable authoritative failure was recorded.
pub(crate) async fn recover_recorded_payment_failure_with_lease(
    stale_saga: &Saga,
    quote: &mut MeltQuote,
    db: &cdk_common::database::DynMintDatabase,
    pubsub: &PubSubManager,
) -> Result<(), Error> {
    let current_saga = if stale_saga.quote_id().is_some() {
        db.get_melt_saga_by_quote_id(&quote.id).await?
    } else {
        let mut tx = db.begin_transaction().await?;
        let reloaded = tx.get_saga(&stale_saga.operation_id).await?;
        tx.rollback().await?;
        reloaded
    };
    let Some(saga) = current_saga else {
        *quote = db
            .get_melt_quote(&quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;
        return Ok(());
    };

    if saga.operation_id != stale_saga.operation_id
        || saga.state != SagaStateEnum::Melt(MeltSagaState::PaymentFailed)
    {
        tracing::warn!(
            "Ignoring negative payment observation for melt quote {} because saga {} is {}",
            quote.id,
            saga.operation_id,
            saga.state.state()
        );
        return Ok(());
    }

    let input_ys = db.get_proof_ys_by_operation_id(&saga.operation_id).await?;
    let blinded_secrets = db
        .get_blinded_secrets_by_operation_id(&saga.operation_id)
        .await?;
    let rollback = super::melt::shared::rollback_failed_melt_quote(
        db,
        pubsub,
        &quote.id,
        &input_ys,
        &blinded_secrets,
        &saga.operation_id,
        &quote.melt_lock,
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

    Ok(())
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
    let owned = tx
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?;
    check_melt_lease(tx, &owned, &quote.melt_lock).await?;
    let mut current_saga = tx
        .get_saga_for_update(&saga.operation_id)
        .await?
        .ok_or(Error::Internal)?;

    match &current_saga.state {
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted) => {
            tx.update_acquired_saga(
                &mut current_saga,
                SagaStateEnum::Melt(MeltSagaState::PaymentPending),
            )
            .await?;
        }
        SagaStateEnum::Melt(MeltSagaState::PaymentPending) => {}
        SagaStateEnum::Melt(
            MeltSagaState::SetupComplete
            | MeltSagaState::PaymentFailed
            | MeltSagaState::Finalizing(_),
        )
        | SagaStateEnum::Swap(_) => {
            tracing::info!(
                "Ignoring pending payment result for melt quote {} because saga {} is {}",
                quote.id,
                saga.operation_id,
                current_saga.state.state()
            );
            return Ok(());
        }
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

    // Convert total_spent to quote unit if possible. Even on unit mismatch, we
    // still persist the raw paid handoff in `Finalizing(Some(_))` before
    // returning `Error::UnitMismatch` so subsequent recovery cannot release
    // reserved proofs.
    let converted_total_spent =
        super::melt::shared::total_spent_for_quote_unit(&payment_response.total_spent, &quote.unit);

    let mut tx = db.begin_transaction().await?;
    let owned = tx
        .get_melt_quote(&quote.id)
        .await?
        .ok_or(Error::UnknownQuote)?;
    check_melt_lease(&mut tx, &owned, &quote.melt_lock).await?;

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

    let (final_total_spent, final_payment_proof, final_lookup_id) = match &acquired_saga.state {
        SagaStateEnum::Melt(MeltSagaState::Finalizing(Some(existing))) => {
            let existing = existing.clone();
            tx.rollback().await?;
            let existing_spent =
                super::melt::shared::total_spent_for_quote_unit(&existing.total_spent, &quote.unit)
                    .map_err(|e| {
                        tracing::error!(
                            "Failed to convert persisted total_spent for quote {}: {:?}",
                            quote.id,
                            e
                        );
                        Error::UnitMismatch
                    })?;
            (
                existing_spent,
                existing.payment_proof,
                existing.payment_lookup_id,
            )
        }
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed) | SagaStateEnum::Swap(_) => {
            tracing::error!(
                "Refusing paid finalization for melt quote {} because saga {} is {}",
                quote.id,
                saga.operation_id,
                acquired_saga.state.state()
            );
            tx.rollback().await?;
            return Err(Error::UnknownPaymentState);
        }
        SagaStateEnum::Melt(
            MeltSagaState::SetupComplete
            | MeltSagaState::PaymentAttempted
            | MeltSagaState::PaymentPending
            | MeltSagaState::Finalizing(None),
        ) => {
            let handoff_total_spent = match &converted_total_spent {
                Ok(spent) => spent.clone(),
                Err(_) => payment_response.total_spent.clone(),
            };
            let finalization_data = MeltFinalizationData {
                total_spent: handoff_total_spent,
                payment_lookup_id: payment_response.payment_lookup_id.clone(),
                payment_proof: payment_response.payment_proof.clone(),
            };
            tx.update_acquired_saga(
                &mut acquired_saga,
                SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data)),
            )
            .await?;
            tx.commit().await?;

            let total_spent = converted_total_spent.map_err(|e| {
                tracing::error!(
                    "Failed to convert recovered total_spent for quote {}: {:?}",
                    quote.id,
                    e
                );
                Error::UnitMismatch
            })?;
            (
                total_spent,
                payment_response.payment_proof.clone(),
                payment_response.payment_lookup_id.clone(),
            )
        }
    };

    // finalize_melt_quote handles the rest of the atomic cleanup:
    // operation recording, saga deletion, and melt request cleanup.
    super::melt::shared::finalize_melt_quote(
        mint,
        db,
        pubsub,
        quote,
        final_total_spent,
        final_payment_proof.clone(),
        &final_lookup_id,
        Some(saga.operation_id),
    )
    .await?;

    // Reflect the finalized state in the caller's in-memory copy so responses
    // built from it are accurate.
    quote.state = MeltQuoteState::Paid;
    quote.payment_proof = final_payment_proof;
    quote.request_lookup_id = Some(final_lookup_id);

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
    use crate::mint::melt::melt_saga::{InternalSettlementOutcome, MeltSaga};
    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};

    fn into_mint_arc<M>(mint: M) -> std::sync::Arc<Mint>
    where
        M: Into<std::sync::Arc<Mint>>,
    {
        mint.into()
    }

    async fn advance_melt_saga_to(
        mint: &crate::mint::Mint,
        operation_id: &uuid::Uuid,
        target: MeltSagaState,
    ) {
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut acquired = tx
            .get_saga_for_update(operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        if matches!(
            acquired.state,
            SagaStateEnum::Melt(MeltSagaState::SetupComplete)
        ) && matches!(
            target,
            MeltSagaState::PaymentPending | MeltSagaState::PaymentFailed
        ) {
            tx.update_acquired_saga(
                &mut acquired,
                SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
            )
            .await
            .unwrap();
        }
        tx.update_acquired_saga(&mut acquired, SagaStateEnum::Melt(target))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    #[test]
    fn test_plan_melt_recovery_exhaustive_matrix() {
        let data = MeltFinalizationData {
            total_spent: Amount::from(100).with_unit(CurrencyUnit::Sat),
            payment_lookup_id: PaymentIdentifier::CustomId("lookup".to_string()),
            payment_proof: Some("preimage".to_string()),
        };

        let all_quote_states = [
            MeltQuoteState::Unpaid,
            MeltQuoteState::Pending,
            MeltQuoteState::Paid,
            MeltQuoteState::Failed,
            MeltQuoteState::Unknown,
        ];

        for quote_state in all_quote_states {
            let expected_setup = match quote_state {
                MeltQuoteState::Pending | MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                    MeltRecoveryStep::CompensateSetup
                }
                MeltQuoteState::Paid | MeltQuoteState::Unknown => MeltRecoveryStep::Noop,
            };
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::SetupComplete, quote_state),
                expected_setup
            );
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::PaymentAttempted, quote_state),
                MeltRecoveryStep::ResolveAttemptedPayment
            );
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::PaymentPending, quote_state),
                MeltRecoveryStep::ResolveAttemptedPayment
            );
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::PaymentFailed, quote_state),
                MeltRecoveryStep::CompensateRecordedFailure
            );
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::Finalizing(Some(data.clone())), quote_state),
                MeltRecoveryStep::Finalize(&data)
            );
            assert_eq!(
                plan_melt_recovery(&MeltSagaState::Finalizing(None), quote_state),
                MeltRecoveryStep::ResolveLegacyFinalizing
            );
        }
    }

    /// A negative observation must not trigger a second backend check: the
    /// status is an authoritative terminal outcome by contract, so the melt
    /// is compensated without re-querying the (here hanging) backend.
    #[tokio::test]
    async fn negative_outcome_compensates_without_a_second_backend_check() {
        use std::sync::Arc;
        use std::time::Duration;

        use cdk_common::payment::{self, MintPayment};

        use crate::mint::{MintBuilder, MintMeltLimits};
        use crate::types::{FeeReserve, QuoteTTL};

        // Backend whose status check never answers. The outcome processor must
        // not call it for a negative observation.
        struct HangingStatusBackend {
            inner: cdk_fake_wallet::FakeWallet,
        }

        #[async_trait::async_trait]
        impl MintPayment for HangingStatusBackend {
            type Err = payment::Error;

            async fn get_settings(&self) -> Result<payment::SettingsResponse, Self::Err> {
                self.inner.get_settings().await
            }
            async fn create_incoming_payment_request(
                &self,
                options: payment::IncomingPaymentOptions,
            ) -> Result<payment::CreateIncomingPaymentResponse, Self::Err> {
                self.inner.create_incoming_payment_request(options).await
            }
            async fn get_payment_quote(
                &self,
                unit: &CurrencyUnit,
                options: payment::OutgoingPaymentOptions,
            ) -> Result<payment::PaymentQuoteResponse, Self::Err> {
                self.inner.get_payment_quote(unit, options).await
            }
            async fn make_payment(
                &self,
                unit: &CurrencyUnit,
                options: payment::OutgoingPaymentOptions,
            ) -> Result<MakePaymentResponse, Self::Err> {
                self.inner.make_payment(unit, options).await
            }
            async fn check_incoming_payment_status(
                &self,
                payment_identifier: &PaymentIdentifier,
            ) -> Result<Vec<payment::WaitPaymentResponse>, Self::Err> {
                self.inner
                    .check_incoming_payment_status(payment_identifier)
                    .await
            }
            async fn check_outgoing_payment(
                &self,
                _payment_identifier: &PaymentIdentifier,
            ) -> Result<MakePaymentResponse, Self::Err> {
                std::future::pending::<()>().await;
                unreachable!("pending never resolves")
            }
            async fn wait_payment_event(
                &self,
            ) -> Result<
                std::pin::Pin<Box<dyn futures::Stream<Item = payment::Event> + Send>>,
                Self::Err,
            > {
                Ok(Box::pin(futures::stream::pending()))
            }
            fn is_payment_event_stream_active(&self) -> bool {
                false
            }
            fn cancel_payment_event_stream(&self) {}
        }

        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut mint_builder = MintBuilder::new(db.clone());
        let backend = HangingStatusBackend {
            inner: cdk_fake_wallet::FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 1.0,
                },
                std::collections::HashMap::default(),
                std::collections::HashSet::default(),
                2,
                CurrencyUnit::Sat,
            ),
        };
        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(backend),
            )
            .await
            .unwrap();
        let mnemonic = bip39::Mnemonic::generate(12).unwrap();
        let mint = mint_builder
            .with_name("test mint".to_string())
            .with_description("test mint".to_string())
            .with_urls(vec!["https://test-mint".to_string()])
            .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);
        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            Arc::new(mint.clone()),
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

        // Park the saga as a backend-acknowledged payment.
        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentPending).await;

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
            status: MeltQuoteState::Failed,
            total_spent: quote.amount(),
        };

        let started = std::time::Instant::now();
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
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
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "negative polling unexpectedly called the hanging backend (elapsed {:?})",
            elapsed
        );
        // The terminal status is authoritative: the melt is compensated, the
        // saga is gone, and the proofs are released back to the user.
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, None).await;
    }

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
        let payment_saga = match setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap()
        {
            InternalSettlementOutcome::RequiresExternalPayment(saga) => saga,
            InternalSettlementOutcome::Settled(_) => panic!("Expected external payment"),
        };
        let _confirmed_saga = payment_saga.make_payment().await.unwrap();

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

        crate::test_helpers::mint::expire_melt_leases(&mint).await;

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
    async fn test_recorded_payment_failure_rolls_back_and_deletes_saga() {
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

        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentFailed).await;

        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        mint.recover_from_incomplete_melt_sagas().await.unwrap();

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

    /// An `Unknown` poll is not an authoritative outcome (e.g. CLN reports it
    /// whenever a payment may still settle), so it must keep the melt
    /// reserved from either dispatch marker; only a later authoritative
    /// failure may compensate it.
    #[tokio::test]
    async fn test_unknown_poll_waits_for_permanent_failure_event() {
        for saga_state in [
            MeltSagaState::PaymentAttempted,
            MeltSagaState::PaymentPending,
        ] {
            let fake_description = FakeInvoiceDescription {
                pay_invoice_state: MeltQuoteState::Unknown,
                check_payment_state: MeltQuoteState::Unknown,
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
                    MeltQuoteState::Unknown,
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

            advance_melt_saga_to(&mint, &operation_id, saga_state.clone()).await;

            let saga = assert_saga_exists(&mint, &operation_id).await;
            let mut quote = mint
                .localstore
                .get_melt_quote(&quote.id)
                .await
                .unwrap()
                .expect("quote should exist");
            let lookup_id = quote
                .request_lookup_id
                .clone()
                .expect("bolt11 quote should have a lookup id");
            let unknown_response = MakePaymentResponse {
                payment_lookup_id: lookup_id.clone(),
                payment_proof: None,
                status: MeltQuoteState::Unknown,
                total_spent: quote.amount(),
            };

            crate::test_helpers::mint::expire_melt_leases(&mint).await;

            process_melt_saga_outcome(
                &saga,
                &mut quote,
                &unknown_response,
                &mint.localstore,
                &mint.pubsub_manager,
                &mint,
            )
            .await
            .unwrap();

            let stored_saga = assert_saga_exists(&mint, &operation_id).await;
            assert_eq!(stored_saga.state, SagaStateEnum::Melt(saga_state.clone()));
            assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;
            assert_eq!(quote.state, MeltQuoteState::Pending);

            let failed_response = MakePaymentResponse {
                payment_lookup_id: lookup_id,
                payment_proof: None,
                status: MeltQuoteState::Failed,
                total_spent: quote.amount(),
            };
            crate::test_helpers::mint::expire_melt_leases(&mint).await;
            process_melt_saga_failure_event(
                &stored_saga,
                &mut quote,
                &failed_response,
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

    /// A terminal `Unpaid`/`Failed` poll is an authoritative outcome by
    /// backend contract: the negative observation is persisted as
    /// `PaymentFailed` and the melt is compensated from either dispatch
    /// marker, without waiting for a permanent-failure event.
    #[tokio::test]
    async fn test_terminal_poll_compensates_dispatched_payment() {
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

                advance_melt_saga_to(&mint, &operation_id, saga_state.clone()).await;

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

                crate::test_helpers::mint::expire_melt_leases(&mint).await;

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
                assert_proofs_state(&mint, &input_ys, None).await;
                assert_eq!(quote.state, MeltQuoteState::Unpaid);

                let persisted_quote = mint
                    .localstore
                    .get_melt_quote(&quote.id)
                    .await
                    .unwrap()
                    .expect("quote should still exist after rollback");
                assert_eq!(persisted_quote.state, quote.state);
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
        let payment_saga = match setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap()
        {
            InternalSettlementOutcome::RequiresExternalPayment(saga) => saga,
            InternalSettlementOutcome::Settled(_) => panic!("Expected external payment"),
        };
        let confirmed_saga = match payment_saga.make_payment().await.unwrap() {
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

        let saga = Saga::new_melt(
            operation_id,
            MeltSagaState::PaymentPending,
            quote.id.to_string(),
        );
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
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
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

        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentPending).await;

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
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
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
        // saga to Finalizing, but does NOT finalize (proofs Pending).
        let confirmed_saga = match setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap()
        {
            InternalSettlementOutcome::Settled(saga) => saga,
            InternalSettlementOutcome::RequiresExternalPayment(_) => {
                panic!("Expected internal settlement")
            }
        };

        // Simulate a crash / interleaving before finalize.
        drop(confirmed_saga);

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

        crate::test_helpers::mint::expire_melt_leases(&mint).await;

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
        // The negative observation is authoritative, but persisting the
        // failure must reach the Finalizing guard without changing the
        // already-finalizing saga.
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

        // Move the saga to PaymentPending and load a stale snapshot of it.
        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentPending).await;
        let stale_saga = assert_saga_exists(&mint, &operation_id).await;

        // A concurrent finalizer commits the Finalizing handoff.
        let finalization_data = MeltFinalizationData {
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
            payment_lookup_id: PaymentIdentifier::CustomId("paid_lookup".to_string()),
            payment_proof: Some("paid_preimage".to_string()),
        };
        advance_melt_saga_to(
            &mint,
            &operation_id,
            MeltSagaState::finalizing(finalization_data.clone()),
        )
        .await;

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
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
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
            SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data))
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
        advance_melt_saga_to(
            &mint,
            &operation_id,
            MeltSagaState::finalizing(finalization_data.clone()),
        )
        .await;

        let payment_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("pending_outcome_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: quote.amount(),
        };

        crate::test_helpers::mint::expire_melt_leases(&mint).await;

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
            SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone()))
        );
        assert_eq!(persisted_saga.finalization_data(), Some(&finalization_data));
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

        crate::test_helpers::mint::expire_melt_leases(&mint).await;

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

        crate::test_helpers::mint::expire_melt_leases(&mint).await;

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

        let persisted_saga = assert_saga_exists(&mint, &operation_id).await;
        assert!(matches!(
            persisted_saga.state,
            SagaStateEnum::Melt(MeltSagaState::Finalizing(Some(_)))
        ));
        assert_eq!(
            persisted_saga.finalization_data().unwrap().total_spent,
            Amount::from(9_250).with_unit(CurrencyUnit::Usd)
        );
    }

    #[tokio::test]
    async fn test_on_demand_poll_unit_mismatch_persists_paid_handoff_before_error() {
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Paid,
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
                MeltQuoteState::Paid,
                Amount::from(9_250).with_unit(CurrencyUnit::Usd),
            ),
        )]);
        let mint = into_mint_arc(
            create_test_mint_with_payment_states(payment_states)
                .await
                .unwrap(),
        );
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
            std::sync::Arc::clone(&mint),
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

        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentPending).await;
        crate::test_helpers::mint::expire_melt_leases(&mint).await;

        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let err = mint
            .handle_pending_melt_quote(&mut quote)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnitMismatch));

        let persisted_saga = assert_saga_exists(&mint, &operation_id).await;
        assert!(matches!(
            persisted_saga.state,
            SagaStateEnum::Melt(MeltSagaState::Finalizing(Some(_)))
        ));
        assert_eq!(
            persisted_saga.finalization_data().unwrap().total_spent,
            Amount::from(9_250).with_unit(CurrencyUnit::Usd)
        );
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;
    }

    #[tokio::test]
    async fn test_legacy_finalizing_backend_error_propagates_on_demand_and_skips_on_startup() {
        use std::sync::Arc;

        use crate::mint::{MintBuilder, MintMeltLimits};
        use crate::types::{FeeReserve, QuoteTTL};

        let db_path = std::env::temp_dir().join(format!(
            "cdk-legacy-finalizing-err-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(
            cdk_sqlite::mint::MintSqliteDatabase::new(&db_path)
                .await
                .unwrap(),
        );

        let invoice_failing = create_fake_invoice(
            Amount::from(9_000).into(),
            serde_json::to_string(&FakeInvoiceDescription {
                pay_invoice_state: MeltQuoteState::Paid,
                check_payment_state: MeltQuoteState::Paid,
                pay_err: false,
                check_err: true,
            })
            .unwrap(),
        );
        let backend = cdk_fake_wallet::FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 1.0,
            },
            std::collections::HashMap::default(),
            std::collections::HashSet::from([invoice_failing.payment_hash().to_string()]),
            2,
            CurrencyUnit::Sat,
        );
        let mut mint_builder = MintBuilder::new(db.clone());
        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(backend),
            )
            .await
            .unwrap();
        let mnemonic = bip39::Mnemonic::generate(12).unwrap();
        let mint = into_mint_arc(
            mint_builder
                .with_name("test mint".to_string())
                .with_description("test mint".to_string())
                .with_urls(vec!["https://test-mint".to_string()])
                .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
                .await
                .unwrap(),
        );
        mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        // Saga 1: legacy Finalizing(None) whose backend check fails.
        let proofs1 = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let quote1_resp = mint
            .get_melt_quote(cdk_common::melt::MeltQuoteRequest::Bolt11(
                MeltQuoteBolt11Request {
                    request: invoice_failing,
                    unit: CurrencyUnit::Sat,
                    options: None,
                },
            ))
            .await
            .unwrap();
        let quote1 = mint
            .localstore
            .get_melt_quote(quote1_resp.quote().unwrap())
            .await
            .unwrap()
            .unwrap();
        let req1 = create_test_melt_request(&proofs1, &quote1);
        let ver1 = mint.verify_inputs(req1.inputs()).await.unwrap();
        let setup1 = MeltSaga::new(Arc::clone(&mint), mint.localstore(), mint.pubsub_manager())
            .setup_melt(&req1, ver1, PaymentMethod::Known(KnownMethod::Bolt11))
            .await
            .unwrap();
        let op1 = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup1);
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE saga_state SET state = 'finalizing', finalization_data = NULL WHERE operation_id = ?1",
                [op1.to_string()],
            )
            .unwrap();
        }

        // On-demand status check must propagate the backend error.
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        let mut quote1_mut = mint
            .localstore
            .get_melt_quote(&quote1.id)
            .await
            .unwrap()
            .unwrap();
        let err = mint
            .handle_pending_melt_quote(&mut quote1_mut)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Internal));

        // Saga 2: SetupComplete saga that startup recovery must still recover
        // after logging and skipping Saga 1's backend error.
        let proofs2 = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys2 = proofs2.ys().unwrap();
        let quote2 = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let req2 = create_test_melt_request(&proofs2, &quote2);
        let ver2 = mint.verify_inputs(req2.inputs()).await.unwrap();
        let setup2 = MeltSaga::new(Arc::clone(&mint), mint.localstore(), mint.pubsub_manager())
            .setup_melt(&req2, ver2, PaymentMethod::Known(KnownMethod::Bolt11))
            .await
            .unwrap();
        let op2 = mint
            .localstore
            .get_melt_saga_by_quote_id(&quote2.id)
            .await
            .unwrap()
            .unwrap()
            .operation_id;
        drop(setup2);

        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        mint.recover_from_incomplete_melt_sagas().await.unwrap();

        // Saga 1 remains for later retry; Saga 2 was compensated.
        assert_saga_exists(&mint, &op1).await;
        assert_saga_not_exists(&mint, &op2).await;
        assert_proofs_state(&mint, &input_ys2, None).await;

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_legacy_finalizing_external_persists_recovered_payload_across_retry() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;

        use cdk_common::payment::{self, MintPayment};
        use cdk_signatory::signatory::Signatory;

        use crate::mint::{MintBuilder, MintMeltLimits};
        use crate::test_helpers::mint::create_test_blinded_messages;
        use crate::types::{FeeReserve, QuoteTTL};

        struct OneShotPaidBackend {
            inner: cdk_fake_wallet::FakeWallet,
            check_calls: Arc<AtomicUsize>,
            paid_response: MakePaymentResponse,
        }

        #[async_trait::async_trait]
        impl MintPayment for OneShotPaidBackend {
            type Err = payment::Error;

            async fn get_settings(&self) -> Result<payment::SettingsResponse, Self::Err> {
                self.inner.get_settings().await
            }
            async fn create_incoming_payment_request(
                &self,
                options: payment::IncomingPaymentOptions,
            ) -> Result<payment::CreateIncomingPaymentResponse, Self::Err> {
                self.inner.create_incoming_payment_request(options).await
            }
            async fn get_payment_quote(
                &self,
                unit: &CurrencyUnit,
                options: payment::OutgoingPaymentOptions,
            ) -> Result<payment::PaymentQuoteResponse, Self::Err> {
                self.inner.get_payment_quote(unit, options).await
            }
            async fn make_payment(
                &self,
                unit: &CurrencyUnit,
                options: payment::OutgoingPaymentOptions,
            ) -> Result<MakePaymentResponse, Self::Err> {
                self.inner.make_payment(unit, options).await
            }
            async fn check_incoming_payment_status(
                &self,
                payment_identifier: &PaymentIdentifier,
            ) -> Result<Vec<payment::WaitPaymentResponse>, Self::Err> {
                self.inner
                    .check_incoming_payment_status(payment_identifier)
                    .await
            }
            async fn check_outgoing_payment(
                &self,
                _payment_identifier: &PaymentIdentifier,
            ) -> Result<MakePaymentResponse, Self::Err> {
                if self.check_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(self.paid_response.clone())
                } else {
                    Err(payment::Error::Custom(
                        "backend unavailable on retry".to_string(),
                    ))
                }
            }
            async fn wait_payment_event(
                &self,
            ) -> Result<
                std::pin::Pin<Box<dyn futures::Stream<Item = payment::Event> + Send>>,
                Self::Err,
            > {
                Ok(Box::pin(futures::stream::pending()))
            }
            fn is_payment_event_stream_active(&self) -> bool {
                false
            }
            fn cancel_payment_event_stream(&self) {}
        }

        struct FailOnceSignatory {
            inner: Arc<dyn Signatory + Send + Sync>,
            fail_next_sign: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl Signatory for FailOnceSignatory {
            fn name(&self) -> String {
                self.inner.name()
            }
            async fn blind_sign(
                &self,
                blinded_messages: Vec<cdk_common::BlindedMessage>,
            ) -> Result<Vec<cdk_common::BlindSignature>, cdk_common::Error> {
                if self.fail_next_sign.swap(false, Ordering::SeqCst) {
                    return Err(cdk_common::Error::Internal);
                }
                self.inner.blind_sign(blinded_messages).await
            }
            async fn verify_proofs(
                &self,
                proofs: Vec<cdk_common::Proof>,
            ) -> Result<(), cdk_common::Error> {
                self.inner.verify_proofs(proofs).await
            }
            async fn keysets(
                &self,
            ) -> Result<cdk_signatory::signatory::SignatoryKeysets, cdk_common::Error> {
                self.inner.keysets().await
            }
            async fn rotate_keyset(
                &self,
                args: cdk_signatory::signatory::RotateKeyArguments,
            ) -> Result<cdk_signatory::signatory::SignatoryKeySet, cdk_common::Error> {
                self.inner.rotate_keyset(args).await
            }
            async fn subscribe_keysets(
                &self,
            ) -> Result<
                tokio::sync::watch::Receiver<cdk_signatory::signatory::SignatoryKeysets>,
                cdk_common::Error,
            > {
                self.inner.subscribe_keysets().await
            }
            async fn reconstruct_dleq(
                &self,
                args: Vec<cdk_signatory::signatory::ReconstructDleqArguments>,
            ) -> Result<Vec<cdk_common::BlindSignature>, cdk_common::Error> {
                self.inner.reconstruct_dleq(args).await
            }
        }

        let db_path = std::env::temp_dir().join(format!(
            "cdk-legacy-finalizing-retry-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(
            cdk_sqlite::mint::MintSqliteDatabase::new(&db_path)
                .await
                .unwrap(),
        );
        let mnemonic = bip39::Mnemonic::generate(12).unwrap();
        let inner_signatory = Arc::new(
            cdk_signatory::db_signatory::DbSignatory::new(
                db.clone(),
                &mnemonic.to_seed_normalized(""),
                vec![(CurrencyUnit::Sat, (0, (0..32).map(|i| 1u64 << i).collect()))]
                    .into_iter()
                    .collect(),
                std::collections::HashMap::new(),
            )
            .await
            .unwrap(),
        );
        let fail_next_sign = Arc::new(AtomicBool::new(false));
        let signatory = Arc::new(FailOnceSignatory {
            inner: inner_signatory,
            fail_next_sign: Arc::clone(&fail_next_sign),
        });

        let check_calls = Arc::new(AtomicUsize::new(0));
        let paid_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("legacy-ext-lookup".to_string()),
            payment_proof: Some("legacy-ext-preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_250).with_unit(CurrencyUnit::Sat),
        };
        let backend = OneShotPaidBackend {
            inner: cdk_fake_wallet::FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 1.0,
                },
                std::collections::HashMap::default(),
                std::collections::HashSet::default(),
                2,
                CurrencyUnit::Sat,
            ),
            check_calls: Arc::clone(&check_calls),
            paid_response: paid_response.clone(),
        };

        let mut mint_builder = MintBuilder::new(db.clone());
        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(backend),
            )
            .await
            .unwrap();
        let mint = into_mint_arc(
            mint_builder
                .with_name("test mint".to_string())
                .with_description("test mint".to_string())
                .with_urls(vec!["https://test-mint".to_string()])
                .build_with_signatory(signatory)
                .await
                .unwrap(),
        );
        mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let (outputs, _) = create_test_blinded_messages(&mint, Amount::from(1_023))
            .await
            .unwrap();
        let melt_request =
            cdk_common::nuts::MeltRequest::new(quote.id.clone(), proofs, Some(outputs));
        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let setup_saga = MeltSaga::new(Arc::clone(&mint), mint.localstore(), mint.pubsub_manager())
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();
        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        drop(setup_saga);

        // Seed a legacy Finalizing(None) row directly in SQLite.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE saga_state SET state = 'finalizing', finalization_data = NULL WHERE operation_id = ?1",
                [operation_id.to_string()],
            )
            .unwrap();
        }

        // First recovery attempt queries the backend once, persists the recovered
        // payload into Finalizing(Some(_)), and then fails during change signing.
        fail_next_sign.store(true, Ordering::SeqCst);
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        let mut quote_mut = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        assert!(mint
            .handle_pending_melt_quote(&mut quote_mut)
            .await
            .is_err());

        let persisted = assert_saga_exists(&mint, &operation_id).await;
        assert!(matches!(
            persisted.state,
            SagaStateEnum::Melt(MeltSagaState::Finalizing(Some(_)))
        ));
        assert_eq!(
            persisted
                .finalization_data()
                .unwrap()
                .payment_proof
                .as_deref(),
            Some("legacy-ext-preimage")
        );
        assert_eq!(check_calls.load(Ordering::SeqCst), 1);

        // Retry completes from the persisted payload without calling the now-failing backend.
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        let mut quote_mut = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        mint.handle_pending_melt_quote(&mut quote_mut)
            .await
            .unwrap();

        assert_eq!(check_calls.load(Ordering::SeqCst), 1);
        assert_eq!(quote_mut.state, MeltQuoteState::Paid);
        assert_eq!(
            quote_mut.payment_proof.as_deref(),
            Some("legacy-ext-preimage")
        );
        assert_saga_not_exists(&mint, &operation_id).await;
        assert_proofs_state(&mint, &input_ys, Some(State::Spent)).await;

        let change_sigs = mint
            .localstore
            .get_blind_signatures_for_quote(&quote.id)
            .await
            .unwrap();
        assert_eq!(
            Amount::try_sum(change_sigs.iter().map(|sig| sig.amount)).unwrap(),
            Amount::from(750)
        );

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_payment_failed_rejects_late_paid_and_ignores_pending() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let input_ys = proofs.ys().unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            into_mint_arc(mint.clone()),
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

        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentFailed).await;

        let saga = assert_saga_exists(&mint, &operation_id).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let original_lookup_id = quote.request_lookup_id.clone();

        // A stale Pending observation must not overwrite PaymentFailed or mutate lookup_id.
        let pending_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("stale_pending_lookup".to_string()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: quote.amount(),
        };
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &pending_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        let after_pending = assert_saga_exists(&mint, &operation_id).await;
        assert_eq!(
            after_pending.state,
            SagaStateEnum::Melt(MeltSagaState::PaymentFailed)
        );
        let stored_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_quote.request_lookup_id, original_lookup_id);

        // A late Paid observation must be refused once PaymentFailed is durably recorded.
        let paid_response = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId("late_paid_lookup".to_string()),
            payment_proof: Some("late_preimage".to_string()),
            status: MeltQuoteState::Paid,
            total_spent: quote.amount(),
        };
        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        let err = process_melt_saga_outcome(
            &saga,
            &mut quote,
            &paid_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::UnknownPaymentState));

        let after_paid = assert_saga_exists(&mint, &operation_id).await;
        assert_eq!(
            after_paid.state,
            SagaStateEnum::Melt(MeltSagaState::PaymentFailed)
        );
        assert_proofs_state(&mint, &input_ys, Some(State::Pending)).await;
    }

    #[tokio::test]
    async fn test_pending_outcome_advances_payment_attempted_to_payment_pending() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            into_mint_arc(mint.clone()),
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

        advance_melt_saga_to(&mint, &operation_id, MeltSagaState::PaymentAttempted).await;

        let saga = assert_saga_exists(&mint, &operation_id).await;
        let mut quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        let lookup = PaymentIdentifier::CustomId("backend_pending_lookup".to_string());
        let pending_response = MakePaymentResponse {
            payment_lookup_id: lookup.clone(),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: quote.amount(),
        };

        crate::test_helpers::mint::expire_melt_leases(&mint).await;
        process_melt_saga_outcome(
            &saga,
            &mut quote,
            &pending_response,
            &mint.localstore,
            &mint.pubsub_manager,
            &mint,
        )
        .await
        .unwrap();

        let after_pending = assert_saga_exists(&mint, &operation_id).await;
        assert_eq!(
            after_pending.state,
            SagaStateEnum::Melt(MeltSagaState::PaymentPending)
        );
        let stored_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored_quote.request_lookup_id, Some(lookup));
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
