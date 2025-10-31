use std::collections::VecDeque;
use std::sync::Arc;

use cdk_common::amount::to_unit;
use cdk_common::database::mint::MeltRequestInfo;
use cdk_common::database::DynMintDatabase;
use cdk_common::mint::{MeltSagaState, Operation, Saga};
use cdk_common::nuts::MeltQuoteState;
use cdk_common::{Amount, Error, ProofsMethods, PublicKey, QuoteId, State};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use tokio::sync::Mutex;
use tracing::instrument;

use self::compensation::{CompensatingAction, RemoveMeltSetup};
use self::state::{Initial, PaymentConfirmed, SettlementDecision, SetupComplete};
use crate::cdk_payment::MakePaymentResponse;
use crate::mint::subscription::PubSubManager;
use crate::mint::verification::Verification;
use crate::mint::{MeltQuoteBolt11Response, MeltRequest};

mod compensation;
mod state;

#[cfg(test)]
mod tests;

/// Saga pattern implementation for atomic melt operations.
///
/// # Why Use the Saga Pattern for Melt?
///
/// The melt operation is more complex than swap because it involves:
/// 1. Database transactions (setup and finalize)
/// 2. External payment operations (Lightning Network)
/// 3. Uncertain payment states (pending/unknown)
/// 4. Change calculation based on actual payment amount
///
/// Traditional ACID transactions cannot span:
/// 1. Multiple database transactions (TX1: setup, TX2: finalize)
/// 2. External payment operations (LN backend calls)
/// 3. Asynchronous payment confirmation
///
/// The saga pattern solves this by:
/// - Breaking the operation into discrete steps with clear state transitions
/// - Recording compensating actions for each forward step
/// - Automatically rolling back via compensations if any step fails
/// - Handling payment state uncertainty explicitly
///
/// # Transaction Boundaries
///
/// - **TX1 (setup_melt)**: Atomically verifies quote, adds input proofs (pending),
///   adds change output blinded messages, creates melt request tracking record
/// - **Payment (make_payment)**: Non-transactional external LN payment operation
/// - **TX2 (finalize)**: Atomically updates quote state, marks inputs spent,
///   signs change outputs, deletes tracking record
///
/// # Expected Flow
///
/// 1. **setup_melt**: Verifies and reserves inputs, prepares change outputs
///    - Compensation: Removes inputs, outputs, resets quote state if later steps fail
/// 2. **make_payment**: Calls LN backend to make payment
///    - Triggers compensation if payment fails
///    - Special handling for pending/unknown states
/// 3. **finalize**: Commits the melt, issues change, marks complete
///    - Triggers compensation if finalization fails
///    - Clears compensations on success (melt complete)
///
/// # Failure Handling
///
/// If any step fails after setup_melt, all compensating actions are executed in reverse
/// order to restore the database to its pre-melt state. This ensures no partial melts
/// leave the system in an inconsistent state.
///
/// # Payment State Complexity
///
/// Unlike swap, melt must handle uncertain payment states:
/// - **Paid**: Proceed to finalize
/// - **Failed/Unpaid**: Compensate and return error
/// - **Pending/Unknown**: Proofs remain pending, saga cannot complete
///   (current behavior: leave proofs pending, return error for manual intervention)
///
/// # Typestate Pattern
///
/// This saga uses the **typestate pattern** to enforce state transitions at compile-time.
/// Each state (Initial, SetupComplete, PaymentConfirmed) is a distinct type, and operations
/// are only available on the appropriate type:
///
/// ```text
/// MeltSaga<Initial>
///   └─> setup_melt() -> MeltSaga<SetupComplete>
///         ├─> attempt_internal_settlement() -> SettlementDecision (conditional)
///         └─> make_payment(SettlementDecision) -> MeltSaga<PaymentConfirmed>
///               └─> finalize() -> MeltQuoteBolt11Response
/// ```
///
/// **Benefits:**
/// - Invalid state transitions (e.g., `finalize()` before `make_payment()`) won't compile
/// - State-specific data (e.g., payment_result) only exists in the appropriate state type
/// - No runtime state checks or `Option<T>` unwrapping needed
/// - IDE autocomplete only shows valid operations for each state
pub struct MeltSaga<S> {
    mint: Arc<super::Mint>,
    db: DynMintDatabase,
    pubsub: Arc<PubSubManager>,
    /// Compensating actions in LIFO order (most recent first)
    compensations: Arc<Mutex<VecDeque<Box<dyn CompensatingAction>>>>,
    /// Operation for tracking
    operation: Operation,
    /// Tracks if metrics were incremented (for cleanup)
    #[cfg(feature = "prometheus")]
    metrics_incremented: bool,
    /// State-specific data
    state_data: S,
}

impl MeltSaga<Initial> {
    pub fn new(mint: Arc<super::Mint>, db: DynMintDatabase, pubsub: Arc<PubSubManager>) -> Self {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("melt_bolt11");

        Self {
            mint,
            db,
            pubsub,
            compensations: Arc::new(Mutex::new(VecDeque::new())),
            operation: Operation::new_melt(),
            #[cfg(feature = "prometheus")]
            metrics_incremented: true,
            state_data: Initial,
        }
    }

    /// Sets up the melt by atomically verifying and reserving inputs/outputs.
    ///
    /// This is the first transaction (TX1) in the saga and must complete before payment.
    ///
    /// # What This Does
    ///
    /// Within a single database transaction:
    /// 1. Verifies the melt request (inputs, quote state, balance)
    /// 2. Adds input proofs to the database with Pending state
    /// 3. Updates quote state from Unpaid/Failed to Pending
    /// 4. Adds change output blinded messages to the database
    /// 5. Creates melt request tracking record
    /// 6. Publishes proof state changes via pubsub
    ///
    /// # Compensation
    ///
    /// Registers a compensation action that will:
    /// - Remove input proofs
    /// - Remove blinded messages
    /// - Reset quote state from Pending to Unpaid
    /// - Delete melt request tracking record
    ///
    /// This compensation runs if payment or finalization fails.
    ///
    /// # Errors
    ///
    /// - `PendingQuote`: Quote is already in Pending state
    /// - `PaidQuote`: Quote has already been paid
    /// - `TokenAlreadySpent`: Input proofs have already been spent
    /// - `UnitMismatch`: Input unit doesn't match quote unit
    #[instrument(skip_all)]
    pub async fn setup_melt(
        self,
        melt_request: &MeltRequest<QuoteId>,
        input_verification: Verification,
    ) -> Result<MeltSaga<SetupComplete>, Error> {
        tracing::info!("TX1: Setting up melt (verify + inputs + outputs)");

        let Verification {
            amount: input_amount,
            unit: input_unit,
        } = input_verification;

        let mut tx = self.db.begin_transaction().await?;

        // Add proofs to the database
        if let Err(err) = tx
            .add_proofs(
                melt_request.inputs().clone(),
                Some(melt_request.quote_id().to_owned()),
                &self.operation,
            )
            .await
        {
            tx.rollback().await?;
            return Err(match err {
                cdk_common::database::Error::Duplicate => Error::TokenPending,
                cdk_common::database::Error::AttemptUpdateSpentProof => Error::TokenAlreadySpent,
                err => Error::Database(err),
            });
        }

        let input_ys = melt_request.inputs().ys()?;

        // Update proof states to Pending
        let original_states = match tx.update_proofs_states(&input_ys, State::Pending).await {
            Ok(states) => states,
            Err(cdk_common::database::Error::AttemptUpdateSpentProof)
            | Err(cdk_common::database::Error::AttemptRemoveSpentProof) => {
                tx.rollback().await?;
                return Err(Error::TokenAlreadySpent);
            }
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        };

        // Check for forbidden states (Pending or Spent)
        let has_forbidden_state = original_states
            .iter()
            .any(|state| matches!(state, Some(State::Pending) | Some(State::Spent)));

        if has_forbidden_state {
            tx.rollback().await?;
            return Err(
                if original_states
                    .iter()
                    .any(|s| matches!(s, Some(State::Pending)))
                {
                    Error::TokenPending
                } else {
                    Error::TokenAlreadySpent
                },
            );
        }

        // Publish proof state changes
        for pk in input_ys.iter() {
            self.pubsub.proof_state((*pk, State::Pending));
        }

        // Update quote state to Pending
        let (state, quote) = tx
            .update_melt_quote_state(melt_request.quote(), MeltQuoteState::Pending, None)
            .await?;

        if input_unit != Some(quote.unit.clone()) {
            tx.rollback().await?;
            return Err(Error::UnitMismatch);
        }

        match state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {}
            MeltQuoteState::Pending => {
                tx.rollback().await?;
                return Err(Error::PendingQuote);
            }
            MeltQuoteState::Paid => {
                tx.rollback().await?;
                return Err(Error::PaidQuote);
            }
            MeltQuoteState::Unknown => {
                tx.rollback().await?;
                return Err(Error::UnknownPaymentState);
            }
        }

        self.pubsub
            .melt_quote_status(&quote, None, None, MeltQuoteState::Pending);

        let fee = self.mint.get_proofs_fee(melt_request.inputs()).await?;

        let required_total = quote.amount + quote.fee_reserve + fee;

        if input_amount < required_total {
            tracing::info!(
                "Melt request unbalanced: inputs {}, amount {}, fee {}",
                input_amount,
                quote.amount,
                fee
            );
            tx.rollback().await?;
            return Err(Error::TransactionUnbalanced(
                input_amount.into(),
                quote.amount.into(),
                (fee + quote.fee_reserve).into(),
            ));
        }

        // Verify outputs if provided
        if let Some(outputs) = &melt_request.outputs() {
            if !outputs.is_empty() {
                let output_verification = match self.mint.verify_outputs(&mut tx, outputs).await {
                    Ok(verification) => verification,
                    Err(err) => {
                        tx.rollback().await?;
                        return Err(err);
                    }
                };

                if input_unit != output_verification.unit {
                    tx.rollback().await?;
                    return Err(Error::UnitMismatch);
                }
            }
        }

        let inputs_fee = self.mint.get_proofs_fee(melt_request.inputs()).await?;

        // Add melt request tracking record
        tx.add_melt_request(
            melt_request.quote_id(),
            melt_request.inputs_amount()?,
            inputs_fee,
        )
        .await?;

        // Add change output blinded messages
        tx.add_blinded_messages(
            Some(melt_request.quote_id()),
            melt_request.outputs().as_ref().unwrap_or(&Vec::new()),
            &self.operation,
        )
        .await?;

        // Get blinded secrets for compensation
        let blinded_secrets: Vec<PublicKey> = melt_request
            .outputs()
            .as_ref()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|bm| bm.blinded_secret)
            .collect();

        // Persist saga state for crash recovery (atomic with TX1)
        let saga = Saga::new_melt(
            *self.operation.id(),
            MeltSagaState::SetupComplete,
            input_ys.clone(),
            blinded_secrets.clone(),
            quote.id.to_string(),
        );

        if let Err(err) = tx.add_saga(&saga).await {
            tx.rollback().await?;
            return Err(err.into());
        }

        tx.commit().await?;

        // Store blinded messages for state
        let blinded_messages_vec = melt_request.outputs().clone().unwrap_or_default();

        // Register compensation (uses LIFO via push_front)
        let compensations = Arc::clone(&self.compensations);
        compensations
            .lock()
            .await
            .push_front(Box::new(RemoveMeltSetup {
                input_ys: input_ys.clone(),
                blinded_secrets,
                quote_id: quote.id.clone(),
            }));

        // Transition to SetupComplete state
        Ok(MeltSaga {
            mint: self.mint,
            db: self.db,
            pubsub: self.pubsub,
            compensations: self.compensations,
            operation: self.operation,
            #[cfg(feature = "prometheus")]
            metrics_incremented: self.metrics_incremented,
            state_data: SetupComplete {
                quote,
                input_ys,
                blinded_messages: blinded_messages_vec,
            },
        })
    }
}

impl MeltSaga<SetupComplete> {
    /// Attempts to settle the melt internally (melt-to-mint on same mint).
    ///
    /// This checks if the payment request corresponds to an existing mint quote
    /// on the same mint, and if so, settles it atomically within a transaction.
    ///
    /// # What This Does
    ///
    /// Within a single database transaction:
    /// 1. Checks if payment request matches a mint quote on this mint
    /// 2. If not a match or different unit: returns (self, RequiresExternalPayment)
    /// 3. If match found: validates quote state and amount
    /// 4. Increments the mint quote's paid amount
    /// 5. Publishes mint quote payment notification
    /// 6. Returns (self, Internal{amount})
    ///
    /// # Compensation
    ///
    /// If internal settlement fails, this method automatically calls compensate_all()
    /// to roll back the setup_melt changes before returning the error. The saga is
    /// consumed on error, so the caller cannot continue.
    ///
    /// # Returns
    ///
    /// - `Ok((self, Internal{amount}))`: Internal settlement succeeded, saga can continue
    /// - `Ok((self, RequiresExternalPayment))`: Not an internal payment, saga can continue
    /// - `Err(_)`: Internal settlement attempted but failed (compensations executed, saga consumed)
    ///
    /// # Errors
    ///
    /// - `RequestAlreadyPaid`: Mint quote already settled
    /// - `InsufficientFunds`: Not enough input proofs for mint quote amount
    /// - `Internal`: Database error during settlement
    #[instrument(skip_all)]
    pub async fn attempt_internal_settlement(
        self,
        melt_request: &MeltRequest<QuoteId>,
    ) -> Result<(Self, SettlementDecision), Error> {
        tracing::info!("Checking for internal settlement opportunity");

        let mut tx = self.db.begin_transaction().await?;

        let mint_quote = match tx
            .get_mint_quote_by_request(&self.state_data.quote.request.to_string())
            .await
        {
            Ok(Some(mint_quote)) if mint_quote.unit == self.state_data.quote.unit => mint_quote,
            Ok(_) => {
                tx.rollback().await?;
                tracing::debug!("Not an internal payment or unit mismatch");
                return Ok((self, SettlementDecision::RequiresExternalPayment));
            }
            Err(err) => {
                tx.rollback().await?;
                tracing::debug!("Error checking for mint quote: {}", err);
                self.compensate_all().await?;
                return Err(Error::Internal);
            }
        };

        // Mint quote has already been settled
        if (mint_quote.state() == cdk_common::nuts::MintQuoteState::Issued
            || mint_quote.state() == cdk_common::nuts::MintQuoteState::Paid)
            && mint_quote.payment_method == crate::mint::PaymentMethod::Bolt11
        {
            tx.rollback().await?;
            self.compensate_all().await?;
            return Err(Error::RequestAlreadyPaid);
        }

        let inputs_amount_quote_unit = melt_request.inputs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        if let Some(amount) = mint_quote.amount {
            if amount > inputs_amount_quote_unit {
                tracing::debug!(
                    "Not enough inputs provided: {} needed {}",
                    inputs_amount_quote_unit,
                    amount
                );
                tx.rollback().await?;
                self.compensate_all().await?;
                return Err(Error::InsufficientFunds);
            }
        }

        let amount = self.state_data.quote.amount;

        tracing::info!(
            "Mint quote {} paid {} from internal payment.",
            mint_quote.id,
            amount
        );

        let total_paid = tx
            .increment_mint_quote_amount_paid(
                &mint_quote.id,
                amount,
                self.state_data.quote.id.to_string(),
            )
            .await?;

        self.pubsub.mint_quote_payment(&mint_quote, total_paid);

        tracing::info!(
            "Melt quote {} paid Mint quote {}",
            self.state_data.quote.id,
            mint_quote.id
        );

        tx.commit().await?;

        Ok((self, SettlementDecision::Internal { amount }))
    }

    /// Makes payment via Lightning Network backend or internal settlement.
    ///
    /// This is an external operation that happens after `setup_melt` and before `finalize`.
    /// No database changes occur in this step (except for internal settlement case).
    ///
    /// # What This Does
    ///
    /// 1. Takes a SettlementDecision from attempt_internal_settlement
    /// 2. If Internal: creates payment result directly
    /// 3. If RequiresExternalPayment: calls LN backend
    /// 4. Handles payment result states with idempotent verification
    /// 5. Transitions to PaymentConfirmed state on success
    ///
    /// # Idempotent Payment Verification
    ///
    /// Lightning payments are asynchronous, and the LN backend may return different
    /// states for the same payment query due to:
    /// - Network latency between payment initiation and confirmation
    /// - Backend database replication lag
    /// - HTLC settlement timing
    ///
    /// **Critical Principle**: If `check_payment_state()` confirms the payment as Paid,
    /// we MUST proceed to finalize, regardless of what `make_payment()` initially returned.
    /// This ensures the saga is idempotent with respect to payment confirmation.
    ///
    /// # Failure Handling
    ///
    /// If payment is confirmed as failed/unpaid, all registered compensations are
    /// executed to roll back the setup transaction.
    ///
    /// # Errors
    ///
    /// - `PaymentFailed`: Payment confirmed as failed/unpaid
    /// - `PendingQuote`: Payment is pending (will be resolved by startup check)
    #[instrument(skip_all)]
    pub async fn make_payment(
        self,
        settlement: SettlementDecision,
    ) -> Result<MeltSaga<PaymentConfirmed>, Error> {
        tracing::info!("Making payment (external LN operation or internal settlement)");

        let payment_result = match settlement {
            SettlementDecision::Internal { amount } => {
                tracing::info!(
                    "Payment settled internally for {} {}",
                    amount,
                    self.state_data.quote.unit
                );
                MakePaymentResponse {
                    status: MeltQuoteState::Paid,
                    total_spent: amount,
                    unit: self.state_data.quote.unit.clone(),
                    payment_proof: None,
                    payment_lookup_id: self
                        .state_data
                        .quote
                        .request_lookup_id
                        .clone()
                        .unwrap_or_else(|| {
                            cdk_common::payment::PaymentIdentifier::CustomId(
                                self.state_data.quote.id.to_string(),
                            )
                        }),
                }
            }
            SettlementDecision::RequiresExternalPayment => {
                // Get LN payment processor
                let ln = self
                    .mint
                    .payment_processors
                    .get(&crate::types::PaymentProcessorKey::new(
                        self.state_data.quote.unit.clone(),
                        self.state_data.quote.payment_method.clone(),
                    ))
                    .ok_or_else(|| {
                        tracing::info!(
                            "Could not get ln backend for {}, {}",
                            self.state_data.quote.unit,
                            self.state_data.quote.payment_method
                        );
                        Error::UnsupportedUnit
                    })?;

                // Make payment with idempotent verification
                let payment_response = match ln
                    .make_payment(
                        &self.state_data.quote.unit,
                        self.state_data.quote.clone().try_into()?,
                    )
                    .await
                {
                    Ok(pay)
                        if pay.status == MeltQuoteState::Unknown
                            || pay.status == MeltQuoteState::Failed =>
                    {
                        tracing::warn!(
                            "Got {} status when paying melt quote {} for {} {}. Verifying with backend...",
                            pay.status,
                            self.state_data.quote.id,
                            self.state_data.quote.amount,
                            self.state_data.quote.unit
                        );

                        let check_response = self
                            .check_payment_state(Arc::clone(ln), &pay.payment_lookup_id)
                            .await?;

                        if check_response.status == MeltQuoteState::Paid {
                            // Race condition: Payment succeeded during verification
                            tracing::info!(
                                "Payment initially returned {} but confirmed as Paid. Proceeding to finalize.",
                                pay.status
                            );
                            check_response
                        } else {
                            check_response
                        }
                    }
                    Ok(pay) => pay,
                    Err(err) => {
                        if matches!(err, crate::cdk_payment::Error::InvoiceAlreadyPaid) {
                            tracing::info!("Invoice already paid, verifying payment status");
                        } else {
                            // Other error - check if payment actually succeeded
                            tracing::error!(
                                "Error returned attempting to pay: {} {}",
                                self.state_data.quote.id,
                                err
                            );
                        }

                        let lookup_id = self
                            .state_data
                            .quote
                            .request_lookup_id
                            .as_ref()
                            .ok_or_else(|| {
                                tracing::error!(
                                "No payment id, cannot verify payment status for {} after error",
                                self.state_data.quote.id
                            );
                                Error::Internal
                            })?;

                        let check_response =
                            self.check_payment_state(Arc::clone(ln), lookup_id).await?;

                        tracing::info!(
                            "Initial payment attempt for {} errored. Follow up check stateus: {}",
                            self.state_data.quote.id,
                            check_response.status
                        );

                        check_response
                    }
                };

                match payment_response.status {
                    MeltQuoteState::Paid => payment_response,
                    MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                        tracing::info!(
                            "Lightning payment for quote {} failed.",
                            self.state_data.quote.id
                        );
                        self.compensate_all().await?;
                        return Err(Error::PaymentFailed);
                    }
                    MeltQuoteState::Unknown => {
                        tracing::warn!(
                            "LN payment unknown, proofs remain pending for quote: {}",
                            self.state_data.quote.id
                        );
                        return Err(Error::PaymentFailed);
                    }
                    MeltQuoteState::Pending => {
                        tracing::warn!(
                            "LN payment pending, proofs remain pending for quote: {}",
                            self.state_data.quote.id
                        );
                        return Err(Error::PendingQuote);
                    }
                }
            }
        };

        // TODO: Add total spent > quote check

        // Transition to PaymentConfirmed state
        Ok(MeltSaga {
            mint: self.mint,
            db: self.db,
            pubsub: self.pubsub,
            compensations: self.compensations,
            operation: self.operation,
            #[cfg(feature = "prometheus")]
            metrics_incremented: self.metrics_incremented,
            state_data: PaymentConfirmed {
                quote: self.state_data.quote,
                input_ys: self.state_data.input_ys,
                blinded_messages: self.state_data.blinded_messages,
                payment_result,
            },
        })
    }

    /// Helper to check payment state with LN backend
    async fn check_payment_state(
        &self,
        ln: Arc<
            dyn cdk_common::payment::MintPayment<Err = cdk_common::payment::Error> + Send + Sync,
        >,
        lookup_id: &cdk_common::payment::PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Error> {
        match ln.check_outgoing_payment(lookup_id).await {
            Ok(response) => Ok(response),
            Err(check_err) => {
                tracing::error!(
                    "Could not check the status of payment for {}. Proofs stuck as pending",
                    lookup_id
                );
                tracing::error!("Checking payment error: {}", check_err);
                Err(Error::Internal)
            }
        }
    }
}

impl MeltSaga<PaymentConfirmed> {
    /// Finalizes the melt by committing signatures and marking inputs as spent.
    ///
    /// This is the second and final transaction (TX2) in the saga and completes the melt.
    ///
    /// # What This Does
    ///
    /// Within a single database transaction:
    /// 1. Updates quote state to Paid
    /// 2. Updates payment lookup ID if changed
    /// 3. Marks input proofs as Spent
    /// 4. Calculates and signs change outputs (if applicable)
    /// 5. Deletes melt request tracking record
    /// 6. Publishes quote status changes via pubsub
    /// 7. Clears all registered compensations (melt successfully completed)
    ///
    /// # Change Handling
    ///
    /// If inputs > total_spent:
    /// - If change outputs were provided: sign them and return
    /// - If no change outputs: change is burnt (logged as info)
    ///
    /// # Success
    ///
    /// On success, compensations are cleared and the melt is complete.
    ///
    /// # Errors
    ///
    /// - `TokenAlreadySpent`: Input proofs were already spent
    /// - `BlindedMessageAlreadySigned`: Change outputs already signed
    #[instrument(skip_all)]
    pub async fn finalize(self) -> Result<MeltQuoteBolt11Response<QuoteId>, Error> {
        tracing::info!("TX2: Finalizing melt (mark spent + change)");

        let total_spent = to_unit(
            self.state_data.payment_result.total_spent,
            &self.state_data.payment_result.unit,
            &self.state_data.quote.unit,
        )
        .unwrap_or_default();

        let payment_preimage = self.state_data.payment_result.payment_proof.clone();
        let payment_lookup_id = &self.state_data.payment_result.payment_lookup_id;

        let mut tx = self.db.begin_transaction().await?;

        // Get melt request info first (needed for validation and change)
        let MeltRequestInfo {
            inputs_amount,
            inputs_fee,
            change_outputs,
        } = tx
            .get_melt_request_and_blinded_messages(&self.state_data.quote.id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Use shared core finalization logic
        if let Err(err) = super::shared::finalize_melt_core(
            &mut tx,
            &self.pubsub,
            &self.state_data.quote,
            &self.state_data.input_ys,
            inputs_amount,
            inputs_fee,
            total_spent,
            payment_preimage.clone(),
            payment_lookup_id,
        )
        .await
        {
            tx.rollback().await?;
            self.compensate_all().await?;
            return Err(err);
        }

        let needs_change = inputs_amount > total_spent;

        // Handle change: either sign change outputs or just commit TX1
        let (change, mut tx) = if !needs_change {
            // No change required - just commit TX1
            tracing::debug!("No change required for melt {}", self.state_data.quote.id);
            (None, tx)
        } else {
            // We commit tx here as process_change can make external call to blind sign
            // We do not want to hold db txs across external calls
            tx.commit().await?;
            super::shared::process_melt_change(
                &self.mint,
                &self.db,
                &self.state_data.quote.id,
                inputs_amount,
                total_spent,
                inputs_fee,
                change_outputs,
            )
            .await?
        };

        tx.delete_melt_request(&self.state_data.quote.id).await?;

        // Delete saga - melt completed successfully (best-effort)
        if let Err(e) = tx.delete_saga(self.operation.id()).await {
            tracing::warn!("Failed to delete saga in finalize: {}", e);
            // Don't rollback - melt succeeded
        }

        tx.commit().await?;

        self.pubsub.melt_quote_status(
            &self.state_data.quote,
            payment_preimage.clone(),
            change.clone(),
            MeltQuoteState::Paid,
        );

        tracing::debug!(
            "Melt for quote {} completed total spent {}, total inputs: {}, change given: {}",
            self.state_data.quote.id,
            total_spent,
            inputs_amount,
            change
                .as_ref()
                .map(|c| Amount::try_sum(c.iter().map(|a| a.amount))
                    .expect("Change cannot overflow"))
                .unwrap_or_default()
        );

        self.compensations.lock().await.clear();

        #[cfg(feature = "prometheus")]
        if self.metrics_incremented {
            METRICS.dec_in_flight_requests("melt_bolt11");
            METRICS.record_mint_operation("melt_bolt11", true);
        }

        let response = MeltQuoteBolt11Response {
            amount: self.state_data.quote.amount,
            paid: Some(true),
            payment_preimage,
            change,
            quote: self.state_data.quote.id,
            fee_reserve: self.state_data.quote.fee_reserve,
            state: MeltQuoteState::Paid,
            expiry: self.state_data.quote.expiry,
            request: Some(self.state_data.quote.request.to_string()),
            unit: Some(self.state_data.quote.unit.clone()),
        };

        Ok(response)
    }
}

impl<S> MeltSaga<S> {
    /// Execute all compensating actions and consume the saga.
    ///
    /// This method takes ownership of self to ensure the saga cannot be used
    /// after compensation has been triggered.
    ///
    /// This is called internally by saga methods when they need to compensate.
    #[instrument(skip_all)]
    async fn compensate_all(self) -> Result<(), Error> {
        let mut compensations = self.compensations.lock().await;

        if compensations.is_empty() {
            return Ok(());
        }

        #[cfg(feature = "prometheus")]
        if self.metrics_incremented {
            METRICS.dec_in_flight_requests("melt_bolt11");
            METRICS.record_mint_operation("melt_bolt11", false);
            METRICS.record_error();
        }

        tracing::warn!("Running {} compensating actions", compensations.len());

        while let Some(compensation) = compensations.pop_front() {
            tracing::debug!("Running compensation: {}", compensation.name());
            if let Err(e) = compensation.execute(&self.db).await {
                tracing::error!(
                    "Compensation {} failed: {}. Continuing...",
                    compensation.name(),
                    e
                );
            }
        }

        Ok(())
    }
}
