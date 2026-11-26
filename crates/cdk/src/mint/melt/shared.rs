//! Shared logic for melt operations across saga and startup check.
//!
//! This module contains common functions used by both:
//! - `melt_saga`: Normal melt operation flow
//! - `start_up_check`: Recovery of interrupted melts during startup
//!
//! The functions here ensure consistency between these two code paths.

use cdk_common::database::mint::Acquired;
#[cfg(feature = "federation")]
use cdk_common::database::mint::FederationJournalEntryKey;
use cdk_common::database::{self, DynMintDatabase, DynMintTransaction};
#[cfg(feature = "federation")]
use cdk_common::federation::{
    FederationJournalEntry, FederationOperationId, FederationSignatureShareRecord,
};
use cdk_common::mint::{self as mint_types};
#[cfg(feature = "federation")]
use cdk_common::nuts::MintQuoteState;
use cdk_common::nuts::{BlindSignature, BlindedMessage, MeltQuoteState, Proofs, State};
use cdk_common::{Amount, CurrencyUnit, Error, PublicKey, QuoteId};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;

#[cfg(feature = "federation")]
use crate::mint::federation::FederationRuntimeConfig;
use crate::mint::subscription::PubSubManager;
use crate::mint::verification::KeysetView;
use crate::mint::MeltQuote;
use crate::Mint;

/// Retrieves fee and amount configuration for the keyset matching the change outputs.
///
/// Looks up the keyset of the first output. Used during change calculation
/// for melts.
///
/// # Returns
///
/// Fee per thousand and allowed amounts for the keyset, or no fee and powers
/// of two if the keyset is unknown or inactive
pub(crate) fn get_keyset_fee_and_amounts<K>(
    keysets: &K,
    outputs: &[BlindedMessage],
) -> cdk_common::amount::FeeAndAmounts
where
    K: KeysetView + ?Sized,
{
    outputs
        .first()
        .and_then(|output| keysets.keyset_info(&output.keyset_id))
        .filter(|keyset| keyset.active)
        .map(|keyset| (keyset.input_fee_ppk, keyset.amounts).into())
        .unwrap_or_else(|| (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into())
}

#[cfg(feature = "prometheus")]
fn amount_as_sats(amount: &Amount<CurrencyUnit>) -> Option<f64> {
    amount.to_msat().ok().map(|msats| msats as f64 / 1000.0)
}

#[cfg(feature = "prometheus")]
fn record_confirmed_payment_metrics(quote: &MeltQuote, total_spent: &Amount<CurrencyUnit>) {
    let payment_method = quote.payment_method.as_str();
    let quote_amount = quote.amount();

    METRICS.record_payment_total(payment_method);

    if let Some(quote_amount_sats) = amount_as_sats(&quote_amount) {
        METRICS.record_payment_amount(payment_method, quote_amount_sats);
    }

    if let Ok(quote_amount) = quote_amount.convert_to(total_spent.unit()) {
        if let Ok(payment_fee) = total_spent.checked_sub(&quote_amount) {
            if let Some(payment_fee_sats) = amount_as_sats(&payment_fee) {
                METRICS.record_payment_fee(payment_method, payment_fee_sats);
            }
        }
    }
}

pub(crate) fn total_spent_for_quote_unit(
    total_spent: &Amount<CurrencyUnit>,
    quote_unit: &CurrencyUnit,
) -> Result<Amount<CurrencyUnit>, Error> {
    total_spent.convert_to_ceil(quote_unit).map_err(Error::from)
}

/// Persist a paid payment result as a durable finalization handoff.
///
/// Once committed, recovery will see `Finalizing` and must never compensate
/// the melt.
pub(crate) async fn persist_melt_finalization_handoff(
    tx: &mut DynMintTransaction,
    saga: &mut Acquired<mint_types::Saga>,
    payment_response: &cdk_common::payment::MakePaymentResponse,
) -> Result<(), Error> {
    let finalization_data = mint_types::MeltFinalizationData {
        total_spent: payment_response.total_spent.clone(),
        payment_lookup_id: payment_response.payment_lookup_id.clone(),
        payment_proof: payment_response.payment_proof.clone(),
    };

    tx.update_acquired_saga_with_finalization_data(
        saga,
        mint_types::SagaStateEnum::Melt(mint_types::MeltSagaState::Finalizing),
        Some(&finalization_data),
    )
    .await?;

    Ok(())
}

/// Rolls back a melt quote by removing all setup artifacts and resetting state.
///
/// This function is used by both:
/// - `melt_saga::compensation::RemoveMeltSetup` when saga fails
/// - `start_up_check::rollback_failed_melt_quote` when recovering failed payments
///
/// # What This Does
///
/// Within a single database transaction:
/// 1. Locks the quote and saga rows (in that order, matching the
///    finalization path's lock order)
/// 2. Verifies the saga still exists and has not advanced to `Finalizing`;
///    a stale compensation whose saga was already rolled back or superseded
///    is a no-op, and a saga already being finalized is never rolled back
/// 3. Removes input proofs from database
/// 4. Removes change output blinded messages
/// 5. Resets quote state from Pending to Unpaid
/// 6. Deletes melt request tracking record
///
/// This restores the database to its pre-melt state, allowing retry.
///
/// # Arguments
///
/// * `db` - Database connection
/// * `quote_id` - ID of the quote to rollback
/// * `input_ys` - Y values (public keys) from input proofs
/// * `blinded_secrets` - Blinded secrets from change outputs
///
/// # Errors
///
/// Returns database errors if transaction fails, `Error::PaidQuote` if the
/// quote is already paid, and `Error::UnknownPaymentState` if the saga has
/// already advanced to `Finalizing` and must not be rolled back.
pub async fn rollback_melt_quote(
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
) -> Result<(), Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(());
    }

    let mut tx = db.begin_transaction().await?;
    tx.lock_quotes(std::slice::from_ref(quote_id)).await?;
    rollback_melt_quote_inner(
        tx,
        pubsub,
        quote_id,
        input_ys,
        blinded_secrets,
        operation_id,
        None,
    )
    .await
}

/// Roll back setup only if the saga still proves payment was never attempted.
pub(crate) async fn rollback_setup_melt_quote(
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
) -> Result<(), Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(());
    }

    let mut tx = db.begin_transaction().await?;
    tx.lock_quotes(std::slice::from_ref(quote_id)).await?;
    rollback_melt_quote_inner(
        tx,
        pubsub,
        quote_id,
        input_ys,
        blinded_secrets,
        operation_id,
        Some(mint_types::MeltSagaState::SetupComplete),
    )
    .await
}

/// Roll back a melt only after an authoritative payment failure was recorded.
pub(crate) async fn rollback_failed_melt_quote(
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
) -> Result<(), Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(());
    }

    let mut tx = db.begin_transaction().await?;
    tx.lock_quotes(std::slice::from_ref(quote_id)).await?;
    rollback_melt_quote_inner(
        tx,
        pubsub,
        quote_id,
        input_ys,
        blinded_secrets,
        operation_id,
        Some(mint_types::MeltSagaState::PaymentFailed),
    )
    .await
}

async fn rollback_melt_quote_inner(
    mut tx: DynMintTransaction,
    pubsub: &PubSubManager,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
    required_saga_state: Option<mint_types::MeltSagaState>,
) -> Result<(), Error> {
    let outcome = rollback_melt_quote_in_transaction(
        &mut tx,
        quote_id,
        input_ys,
        blinded_secrets,
        operation_id,
        required_saga_state,
    )
    .await;

    match outcome {
        Ok(Some(notifications)) => {
            tx.commit().await?;
            notifications.publish(pubsub);
            Ok(())
        }
        Ok(None) => {
            tx.rollback().await?;
            Ok(())
        }
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
}

/// Rolls back a melt quote inside a caller-supplied transaction.
///
/// This performs every write of [`rollback_melt_quote`] but neither commits
/// nor publishes. It returns `Ok(None)` when there is nothing to roll back
/// (no artifacts, or the owning saga no longer exists), and the notifications
/// to publish after the caller commits otherwise.
///
/// The refusals ([`Error::UnknownPaymentState`] and [`Error::PaidQuote`]) are
/// returned before this function writes anything, so a caller may treat them
/// as a refusal and keep using the transaction. Any other error must roll the
/// transaction back.
pub(crate) async fn rollback_melt_quote_in_transaction(
    tx: &mut DynMintTransaction,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
    required_saga_state: Option<mint_types::MeltSagaState>,
) -> Result<Option<MeltNotifications>, Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(None);
    }

    tracing::info!(
        "Rolling back melt quote {} ({} proofs, {} blinded messages, saga {})",
        quote_id,
        input_ys.len(),
        blinded_secrets.len(),
        operation_id
    );

    // Acquire quote locks before touching the saga, melt-request,
    // blinded-signature, or proof rows. Finalization uses the same order.
    let locked_quotes = tx.lock_melt_quote_and_related(quote_id).await?;

    // Ownership and state guard: only the saga that still owns this melt may
    // roll it back. Acquiring the saga locks its row, so a concurrent rollback
    // that already deleted the row, or a concurrent finalizer that already
    // committed `Finalizing`, is observed before any setup artifact is removed.
    match tx.get_saga_for_update(operation_id).await? {
        None => {
            tracing::info!(
                "Skipping rollback for melt quote {} because saga {} no longer exists",
                quote_id,
                operation_id
            );
            return Ok(None);
        }
        Some(saga) => {
            if let Some(required_state) = required_saga_state {
                if saga.state != mint_types::SagaStateEnum::Melt(required_state.clone()) {
                    tracing::info!(
                        "Refusing setup rollback for melt quote {} because saga {} advanced to {}",
                        quote_id,
                        operation_id,
                        saga.state.state()
                    );
                    return Err(Error::UnknownPaymentState);
                }
            }

            if matches!(
                &saga.state,
                mint_types::SagaStateEnum::Melt(mint_types::MeltSagaState::Finalizing)
            ) {
                tracing::warn!(
                    "Refusing rollback for melt quote {}: saga {} is already Finalizing",
                    quote_id,
                    operation_id
                );
                return Err(Error::UnknownPaymentState);
            }
        }
    }

    let quote_option = match locked_quotes.target {
        Some(mut quote) => match quote.state {
            MeltQuoteState::Pending => {
                tx.update_melt_quote_state(&mut quote, MeltQuoteState::Unpaid, None)
                    .await?;
                Some(quote.inner())
            }
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                // Already in a non-pending state; fall through to saga / melt-request
                // cleanup so rollback remains idempotent.
                None
            }
            MeltQuoteState::Paid => {
                return Err(Error::PaidQuote);
            }
            state => {
                tracing::warn!(
                    "Refusing rollback for melt quote {} in unexpected state {}",
                    quote_id,
                    state
                );
                return Err(Error::UnknownPaymentState);
            }
        },
        None => None,
    };

    // Finalization locks melt-request and blinded-signature rows before proofs.
    // Delete them in that same order during rollback.
    tx.delete_melt_request(quote_id).await?;

    // Delete by blinded secret as a defensive cleanup for legacy or incomplete rows
    // that may not have the expected quote association.
    if !blinded_secrets.is_empty() {
        tx.delete_blinded_messages(blinded_secrets).await?;
    }

    let mut proofs_recovered = false;

    // Remove input proofs
    if !input_ys.is_empty() {
        match tx.remove_proofs(input_ys, Some(quote_id.clone())).await {
            Ok(_) => {
                proofs_recovered = true;
            }
            Err(database::Error::AttemptRemoveSpentProof) => {
                tracing::warn!(
                    "Proofs already spent or missing during rollback for quote {}",
                    quote_id
                );
            }
            Err(e) => return Err(e.into()),
        }
    }

    // Delete saga state in the same transaction as proof and quote recovery.
    tx.delete_saga(operation_id).await?;

    #[cfg(all(test, feature = "federation"))]
    if crate::test_helpers::mint::should_fail_for("FEDERATION_MELT_ROLLBACK_BEFORE_COMMIT") {
        return Err(Error::Internal);
    }

    let mut notifications = MeltNotifications::default();
    if proofs_recovered {
        notifications
            .proof_states
            .extend(input_ys.iter().map(|pk| (*pk, State::Unspent)));
    }

    let quote_reset = quote_option.is_some();
    if let Some(quote) = quote_option {
        notifications
            .quote_statuses
            .push(MeltQuoteStatusNotification {
                quote,
                payment_proof: None,
                change: None,
                state: MeltQuoteState::Unpaid,
            });
    }

    tracing::info!(
        quote_id = %quote_id,
        saga_id = %operation_id,
        quote_state_reset_to_unpaid = quote_reset,
        proof_count = input_ys.len(),
        proofs_recovered,
        change_output_count = blinded_secrets.len(),
        "melt rollback staged; reservation cleanup commits with the transaction",
    );

    Ok(Some(notifications))
}

/// Pub/sub notifications produced by a melt state change.
///
/// Melt functions that work inside a caller-supplied transaction return this
/// value instead of publishing. The caller publishes it only after the
/// transaction commits, so subscribers never see state that was rolled back.
#[derive(Debug, Default)]
#[must_use = "melt notifications must be published after the transaction commits"]
pub(crate) struct MeltNotifications {
    proof_states: Vec<(PublicKey, State)>,
    quote_statuses: Vec<MeltQuoteStatusNotification>,
    #[cfg(feature = "prometheus")]
    confirmed_payments: Vec<(MeltQuote, Amount<CurrencyUnit>)>,
}

#[derive(Debug)]
struct MeltQuoteStatusNotification {
    quote: MeltQuote,
    payment_proof: Option<String>,
    change: Option<Vec<BlindSignature>>,
    state: MeltQuoteState,
}

impl MeltNotifications {
    /// Append the notifications of a later state change.
    #[cfg(feature = "federation")]
    pub(crate) fn extend(&mut self, other: Self) {
        self.proof_states.extend(other.proof_states);
        self.quote_statuses.extend(other.quote_statuses);
        #[cfg(feature = "prometheus")]
        self.confirmed_payments.extend(other.confirmed_payments);
    }

    /// Publish the notifications. Call only after the transaction committed.
    pub(crate) fn publish(self, pubsub: &PubSubManager) {
        for proof_state in self.proof_states {
            pubsub.proof_state(proof_state);
        }

        for status in self.quote_statuses {
            if status.state == MeltQuoteState::Paid {
                tracing::info!(
                    quote_id = %status.quote.id,
                    payment_lookup_id = ?status.quote.request_lookup_id,
                    quote_amount = %status.quote.amount(),
                    change_signature_count = status.change.as_ref().map_or(0, Vec::len),
                    "melt finalized; quote is paid and input proofs are spent",
                );
            }
            pubsub.melt_quote_status(
                &status.quote,
                status.payment_proof,
                status.change,
                status.state,
            );
        }

        #[cfg(feature = "prometheus")]
        for (quote, total_spent) in self.confirmed_payments {
            record_confirmed_payment_metrics(&quote, &total_spent);
        }
    }
}

enum MeltCleanupTransaction {
    Ready(Box<dyn database::MintTransaction<database::Error> + Send + Sync>),
    AlreadyCompleted,
}

async fn begin_melt_cleanup_transaction(
    db: &DynMintDatabase,
    quote_id: &QuoteId,
) -> Result<MeltCleanupTransaction, Error> {
    let mut tx = db.begin_transaction().await?;

    // TX1 finalization and rollback both acquire these locks in this order. TX2
    // must do the same so a duplicate finalizer cannot hold melt_request while
    // waiting for blind_signature rows already held by this transaction.
    let locked_quotes = tx.lock_melt_quote_and_related(quote_id).await?;
    if locked_quotes.target.is_none() {
        tx.rollback().await?;
        return Err(Error::UnknownQuote);
    }

    if tx
        .get_melt_request_and_blinded_messages(quote_id)
        .await?
        .is_none()
    {
        // Another finalizer can complete TX2 after this finalizer releases its
        // TX1 locks. Treat the missing request as completed instead of trying
        // to sign change or insert the completed operation a second time.
        tx.rollback().await?;
        return Ok(MeltCleanupTransaction::AlreadyCompleted);
    }

    Ok(MeltCleanupTransaction::Ready(tx))
}

pub(super) enum MeltChangeResult {
    Ready {
        change_sigs: Option<Vec<BlindSignature>>,
        tx: Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    },
    AlreadyCompleted,
}

async fn begin_melt_change_without_signatures(
    db: &DynMintDatabase,
    quote_id: &QuoteId,
) -> Result<MeltChangeResult, Error> {
    Ok(match begin_melt_cleanup_transaction(db, quote_id).await? {
        MeltCleanupTransaction::Ready(tx) => MeltChangeResult::Ready {
            change_sigs: None,
            tx,
        },
        MeltCleanupTransaction::AlreadyCompleted => MeltChangeResult::AlreadyCompleted,
    })
}

/// Processes change for a melt operation.
///
/// This function handles the complete change workflow:
/// 1. Calculate change target amount
/// 2. Split into denominations based on keyset configuration
/// 3. Sign change outputs (external call to blind_sign)
/// 4. Store signatures in database (new transaction)
///
/// # Transaction Management
///
/// This function expects that the caller has already committed or will rollback
/// their current transaction before calling. It will:
/// - Call blind_sign (external, no DB lock held)
/// - Open a new transaction to store signatures
/// - Return the new transaction for the caller to commit
///
/// # Arguments
///
/// * `mint` - Mint instance (for keysets and blind_sign)
/// * `db` - Database connection
/// * `quote_id` - Quote ID for associating signatures
/// * `inputs_amount` - Total amount from input proofs
/// * `total_spent` - Amount spent on payment
/// * `inputs_fee` - Fee paid for inputs
/// * `change_outputs` - Blinded messages for change
///
/// # Returns
///
/// [`MeltChangeResult::Ready`] contains the signed change outputs and the new
/// transaction. [`MeltChangeResult::AlreadyCompleted`] indicates that another
/// finalizer completed cleanup after this finalizer released its initial locks.
///
/// # Errors
///
/// Returns error if:
/// - Change calculation fails
/// - Blind signing fails
/// - Database operations fail
pub(super) async fn process_melt_change(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    quote_id: &QuoteId,
    inputs_amount: Amount<CurrencyUnit>,
    total_spent: Amount<CurrencyUnit>,
    inputs_fee: Amount<CurrencyUnit>,
    change_outputs: Vec<BlindedMessage>,
) -> Result<MeltChangeResult, Error> {
    let change_target: Amount = match inputs_amount
        .checked_sub(&total_spent)
        .ok()
        .and_then(|rem| rem.checked_sub(&inputs_fee).ok())
    {
        Some(amt) if amt.value() > 0 => amt.into(),
        Some(change_target) => {
            tracing::debug!(
                quote_id = %quote_id,
                inputs_amount = %inputs_amount,
                total_spent = %total_spent,
                inputs_fee = %inputs_fee,
                change_target = %change_target,
                "melt has no change to return after payment and input fees",
            );
            return begin_melt_change_without_signatures(db, quote_id).await;
        }
        None => {
            tracing::warn!(
                quote_id = %quote_id,
                inputs_amount = %inputs_amount,
                total_spent = %total_spent,
                inputs_fee = %inputs_fee,
                "payment and input fees exceed the provided melt inputs; no change can be returned",
            );
            return begin_melt_change_without_signatures(db, quote_id).await;
        }
    };

    if change_outputs.is_empty() {
        tracing::info!(
            quote_id = %quote_id,
            change_target = %change_target,
            "melt has refundable change but the wallet supplied no change outputs",
        );
        return begin_melt_change_without_signatures(db, quote_id).await;
    }

    // Get keyset configuration
    let fee_and_amounts = get_keyset_fee_and_amounts(mint, &change_outputs);

    // Split change into denominations
    let mut amounts: Vec<Amount> = change_target.split(&fee_and_amounts)?;

    if change_outputs.len() < amounts.len() {
        tracing::info!(
            quote_id = %quote_id,
            change_target = %change_target,
            required_change_output_count = amounts.len(),
            provided_change_output_count = change_outputs.len(),
            "wallet supplied too few change outputs; returned change may be less than the available amount",
        );
        amounts.sort_by(|a, b| b.cmp(a));
    }

    // Prepare blinded messages with amounts
    let mut blinded_messages_to_sign = vec![];
    for (amount, mut blinded_message) in amounts.iter().zip(change_outputs.iter().cloned()) {
        blinded_message.amount = *amount;
        blinded_messages_to_sign.push(blinded_message);
    }

    // External call: sign change outputs (no DB transaction held)
    let change_sigs = mint
        .blind_sign(blinded_messages_to_sign.clone())
        .await
        .inspect_err(|err| {
            tracing::error!(
                quote_id = %quote_id,
                change_target = %change_target,
                change_output_count = blinded_messages_to_sign.len(),
                error = %err,
                "melt change signing failed; paid melt remains available for finalization recovery",
            );
        })?;

    // Open a transaction with quote, melt-request, and change-output locks
    // acquired in the same order as finalization and rollback.
    let mut tx = match begin_melt_cleanup_transaction(db, quote_id).await? {
        MeltCleanupTransaction::Ready(tx) => tx,
        MeltCleanupTransaction::AlreadyCompleted => {
            return Ok(MeltChangeResult::AlreadyCompleted);
        }
    };

    let blinded_secrets: Vec<_> = blinded_messages_to_sign
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    tx.add_blind_signatures(&blinded_secrets, &change_sigs, Some(quote_id.clone()))
        .await?;

    Ok(MeltChangeResult::Ready {
        change_sigs: Some(change_sigs),
        tx,
    })
}

#[cfg(feature = "federation")]
struct PreparedFederationMeltChange {
    change_sigs: Option<Vec<BlindSignature>>,
    signature_share_records: Vec<FederationSignatureShareRecord>,
    blinded_secrets: Vec<PublicKey>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone)]
pub(crate) struct FederationSignatureShareProvenance {
    entry: FederationJournalEntry,
}

#[cfg(feature = "federation")]
impl FederationSignatureShareProvenance {
    pub(crate) fn new(entry: FederationJournalEntry) -> Self {
        Self { entry }
    }
}

#[cfg(feature = "federation")]
async fn federation_signature_share_provenance_for_operation(
    db: &DynMintDatabase,
    federation_operation_id: FederationOperationId,
) -> Result<FederationSignatureShareProvenance, Error> {
    let entry = db
        .get_federation_journal_entry(
            FederationJournalEntryKey::Operation(federation_operation_id),
            None,
        )
        .await?
        .ok_or_else(|| {
            Error::Custom(
                "federation operation must be accepted in the journal before signing change"
                    .to_string(),
            )
        })?;

    Ok(FederationSignatureShareProvenance::new(entry))
}

#[cfg(feature = "federation")]
async fn federation_signature_share_provenance_in_transaction(
    tx: &mut DynMintTransaction,
    federation_operation_id: FederationOperationId,
) -> Result<FederationSignatureShareProvenance, Error> {
    let entry = tx
        .get_federation_journal_entry(
            FederationJournalEntryKey::Operation(federation_operation_id),
            None,
        )
        .await?
        .ok_or_else(|| {
            Error::Custom(
                "federation operation must be accepted in the journal before signing change"
                    .to_string(),
            )
        })?;

    Ok(FederationSignatureShareProvenance::new(entry))
}

#[cfg(feature = "federation")]
impl PreparedFederationMeltChange {
    fn empty() -> Self {
        Self {
            change_sigs: None,
            signature_share_records: Vec::new(),
            blinded_secrets: Vec::new(),
        }
    }
}

#[cfg(feature = "federation")]
pub(crate) enum FederationInternalMeltSettlement {
    NotInternal,
    Finalized {
        quote: Box<MeltQuote>,
        change_sigs: Option<Vec<BlindSignature>>,
    },
}

#[cfg(feature = "federation")]
#[derive(Debug)]
pub(crate) struct FederationMeltChange {
    pub(crate) inputs_amount: Amount<CurrencyUnit>,
    pub(crate) total_spent: Amount<CurrencyUnit>,
    pub(crate) inputs_fee: Amount<CurrencyUnit>,
    pub(crate) outputs: Vec<BlindedMessage>,
}

#[cfg(feature = "federation")]
fn prepare_federation_melt_change(
    config: &FederationRuntimeConfig,
    quote_id: &QuoteId,
    federation_operation_id: FederationOperationId,
    signature_share_provenance: FederationSignatureShareProvenance,
    change: FederationMeltChange,
) -> Result<PreparedFederationMeltChange, Error> {
    let FederationMeltChange {
        inputs_amount,
        total_spent,
        inputs_fee,
        outputs: change_outputs,
    } = change;
    if signature_share_provenance.entry.operation_id != federation_operation_id {
        return Err(
            cdk_common::federation::FederationError::SignatureShareOperationMismatch {
                expected: federation_operation_id,
                found: signature_share_provenance.entry.operation_id,
            }
            .into(),
        );
    }

    let change_target: Amount = match inputs_amount
        .checked_sub(&total_spent)
        .ok()
        .and_then(|rem| rem.checked_sub(&inputs_fee).ok())
    {
        Some(amt) if amt.value() > 0 => amt.into(),
        Some(_) => return Ok(PreparedFederationMeltChange::empty()),
        None => {
            tracing::warn!(
                "Fee was too high for quote {}. inputs_amount: {}, total_spent: {}, inputs_fee: {}",
                quote_id,
                inputs_amount,
                total_spent,
                inputs_fee
            );
            return Ok(PreparedFederationMeltChange::empty());
        }
    };

    if change_outputs.is_empty() {
        return Ok(PreparedFederationMeltChange::empty());
    }

    let fee_and_amounts = get_keyset_fee_and_amounts(&config.keysets(), &change_outputs);
    let mut amounts: Vec<Amount> = change_target.split(&fee_and_amounts)?;

    if change_outputs.len() < amounts.len() {
        tracing::debug!(
            "Providing federated change requires {} blinded messages, but only {} provided",
            amounts.len(),
            change_outputs.len()
        );
        amounts.sort_by(|a, b| b.cmp(a));
    }

    let mut blinded_messages_to_sign = Vec::with_capacity(amounts.len());
    for (amount, mut blinded_message) in amounts.iter().zip(change_outputs.iter().cloned()) {
        blinded_message.amount = *amount;
        blinded_messages_to_sign.push(blinded_message);
    }

    let share_set = config.sign_accepted_outputs(
        &signature_share_provenance.entry,
        &blinded_messages_to_sign,
        "melt",
    )?;
    let signature_share_records = FederationSignatureShareRecord::from_accepted_journal_entry(
        &share_set,
        &signature_share_provenance.entry,
    )?;
    let change_sigs = crate::mint::federation::federation_share_signatures(share_set);
    let blinded_secrets = blinded_messages_to_sign
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    Ok(PreparedFederationMeltChange {
        change_sigs: Some(change_sigs),
        signature_share_records,
        blinded_secrets,
    })
}

/// Signs federated melt change and stores the shares and signatures in `tx`.
///
/// Federated change shares are computed locally from the accepted journal
/// entry, so no external call is made while the transaction is open.
#[cfg(feature = "federation")]
pub(crate) async fn add_federation_melt_change(
    tx: &mut DynMintTransaction,
    config: &FederationRuntimeConfig,
    quote_id: &QuoteId,
    federation_operation_id: FederationOperationId,
    signature_share_provenance: FederationSignatureShareProvenance,
    change: FederationMeltChange,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    let prepared_change = prepare_federation_melt_change(
        config,
        quote_id,
        federation_operation_id,
        signature_share_provenance,
        change,
    )?;

    if !prepared_change.signature_share_records.is_empty() {
        tx.add_federation_signature_shares(&prepared_change.signature_share_records)
            .await?;
    }

    if let Some(change_sigs) = prepared_change.change_sigs.as_ref() {
        tx.add_blind_signatures(
            &prepared_change.blinded_secrets,
            change_sigs,
            Some(quote_id.clone()),
        )
        .await?;
    }

    Ok(prepared_change.change_sigs)
}

/// Loads a melt quote and acquires exclusive locks on all related quotes.
///
/// This function combines quote loading with defensive locking to prevent race conditions in BOLT12
/// scenarios where multiple melt quotes can share the same `request_lookup_id`. It performs the
/// following operations atomically in a single query:
///
/// 1. Acquires row-level locks on ALL quotes sharing the same lookup identifier (including target)
/// 2. Returns the target quote and validates no sibling is already `Pending` or `Paid`
///
/// # Deadlock Prevention
///
/// This function uses a single atomic query to lock all related quotes at once, ordered by ID.
/// This prevents deadlocks that would occur if we locked the target quote first, then tried to
/// lock related quotes separately - concurrent transactions would each hold one lock and wait
/// for the other, creating a circular wait condition.
///
/// # Arguments
///
/// * `tx` - The active database transaction used to load and acquire locks.
/// * `quote_id` - The ID of the melt quote to load and process.
///
/// # Returns
///
/// The loaded and locked melt quote, ready for state transitions.
///
/// # Errors
///
/// * [`Error::UnknownQuote`] if no quote exists with the given ID.
/// * [`Error::PendingQuote`] (code 20005) if another quote with the same lookup ID is pending.
/// * [`Error::RequestAlreadyPaid`] (code 20006) if another quote with the same lookup ID is paid.
pub async fn load_melt_quotes_exclusively(
    tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    quote_id: &QuoteId,
) -> Result<Acquired<MeltQuote>, Error> {
    // Lock ALL related quotes in a single atomic query to prevent deadlocks.
    // The query locks quotes ordered by ID, ensuring consistent lock acquisition order
    // across concurrent transactions.
    let locked = tx.lock_melt_quote_and_related(quote_id).await?;

    let quote = locked.target.ok_or(Error::UnknownQuote)?;

    // Check if any sibling quote (same lookup_id) is already pending or paid
    if let Some(conflict) = locked.all_related.iter().find(|locked_quote| {
        locked_quote.id != quote.id
            && (locked_quote.state == MeltQuoteState::Pending
                || locked_quote.state == MeltQuoteState::Paid)
    }) {
        tracing::warn!(
            "Cannot transition quote {} to Pending: another quote with lookup_id {:?} is already {:?}",
            quote.id,
            quote.request_lookup_id,
            conflict.state,
        );
        // Return spec-compliant error codes:
        // - 20005 (QuotePending) if sibling is Pending
        // - 20006 (InvoiceAlreadyPaid) if sibling is Paid
        return Err(match conflict.state {
            MeltQuoteState::Pending => Error::PendingQuote,
            MeltQuoteState::Paid => Error::RequestAlreadyPaid,
            _ => unreachable!("Only Pending/Paid states reach this branch"),
        });
    }

    Ok(quote)
}

/// Finalizes a melt quote by updating proofs, quote state, and publishing changes.
///
/// This function performs the core finalization operations that are common to both
/// the saga finalize step and startup check recovery:
/// 1. Validates amounts (total_spent vs quote amount, inputs vs total_spent)
/// 2. Marks input proofs as SPENT
/// 3. Publishes proof state changes
/// 4. Updates quote state to PAID
/// 5. Updates payment lookup ID if changed
/// 6. Deletes melt request tracking
///
/// # Transaction Management
///
/// This function expects an open transaction and will NOT commit it.
/// The caller is responsible for committing the transaction.
///
/// # Arguments
///
/// * `tx` - Open database transaction
/// * `pubsub` - Pubsub manager for state notifications
/// * `quote` - Melt quote being finalized
/// * `input_ys` - Y values of input proofs
/// * `inputs_amount` - Total amount from inputs
/// * `inputs_fee` - Fee for inputs
/// * `total_spent` - Amount spent on payment
/// * `payment_proof` - Payment preimage (if any)
/// * `payment_lookup_id` - Payment lookup identifier
///
/// # Returns
///
/// `Ok(Proofs)` — a clone of the input proofs (now marked Spent), which callers
/// can use to compute the per-keyset fee breakdown for operation recording.
/// The proofs are cloned out of the `Acquired` wrapper so that no database
/// row locks are held after this function returns.
///
/// # Errors
///
/// Returns error if:
/// - Amount validation fails
/// - Proofs are already spent
/// - Database operations fail
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_melt_core(
    mut tx: Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    pubsub: &PubSubManager,
    quote: Acquired<MeltQuote>,
    input_ys: &[PublicKey],
    inputs_amount: Amount<CurrencyUnit>,
    inputs_fee: Amount<CurrencyUnit>,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
) -> Result<(Proofs, MeltQuote), Error> {
    let outcome = finalize_melt_core_in_transaction(
        &mut tx,
        quote,
        input_ys,
        inputs_amount,
        inputs_fee,
        total_spent,
        payment_proof,
        payment_lookup_id,
    )
    .await;

    let finalized = match outcome {
        Ok(finalized) => finalized,
        Err(err) => {
            tx.rollback().await?;
            return Err(err);
        }
    };

    tx.commit().await?;

    // Publish proof state changes
    for pk in input_ys.iter() {
        pubsub.proof_state((*pk, State::Spent));
    }

    Ok(finalized)
}

/// Core melt finalization inside a caller-supplied transaction.
///
/// Performs the writes of [`finalize_melt_core`] (amount validation, quote
/// `Paid`, lookup-id update, inputs `Spent`) without committing, rolling
/// back, or publishing. The caller publishes the spent proof states after it
/// commits.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_melt_core_in_transaction(
    tx: &mut DynMintTransaction,
    mut quote: Acquired<MeltQuote>,
    input_ys: &[PublicKey],
    inputs_amount: Amount<CurrencyUnit>,
    inputs_fee: Amount<CurrencyUnit>,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
) -> Result<(Proofs, MeltQuote), Error> {
    // Validate quote amount vs payment amount
    if quote.amount() > total_spent {
        tracing::error!(
            "Payment amount {} is less than quote amount {} for quote {}",
            total_spent,
            quote.amount(),
            quote.id
        );
        return Err(Error::IncorrectQuoteAmount);
    }

    // Validate inputs amount
    let net_inputs = inputs_amount.checked_sub(&inputs_fee)?;

    // Convert total_spent to the same unit as net_inputs for comparison.
    // Backends should return total_spent in the quote's unit, but we convert defensively.
    let total_spent = total_spent.convert_to_ceil(net_inputs.unit())?;

    tracing::debug!(
        "Melt validation for quote {}: inputs_amount={}, inputs_fee={}, net_inputs={}, total_spent={}, quote_amount={}, fee_reserve={}",
        quote.id,
        inputs_amount.display_with_unit(),
        inputs_fee.display_with_unit(),
        net_inputs.display_with_unit(),
        total_spent.display_with_unit(),
        quote.amount().display_with_unit(),
        quote.fee_reserve().display_with_unit(),
    );

    // This can happen with external payment processors that cannot set a maximum fee,
    // allowing payments to exceed the fee reserve.
    debug_assert!(
        net_inputs >= total_spent,
        "Over paid melt quote {}: net_inputs ({}) < total_spent ({}). Payment already complete, finalizing with no change.",
        quote.id,
        net_inputs.display_with_unit(),
        total_spent.display_with_unit(),
    );
    if net_inputs < total_spent {
        tracing::error!(
            "Over paid melt quote {}: net_inputs ({}) < total_spent ({}). Payment already complete, finalizing with no change.",
            quote.id,
            net_inputs.display_with_unit(),
            total_spent.display_with_unit(),
        );
        // Payment is already done - continue finalization but no change will be returned
    }

    // Update quote state to Paid
    tx.update_melt_quote_state(&mut quote, MeltQuoteState::Paid, payment_proof)
        .await?;

    quote.state = MeltQuoteState::Paid;

    // Update payment lookup ID if changed
    if quote.request_lookup_id.as_ref() != Some(payment_lookup_id) {
        tracing::info!(
            "Payment lookup id changed post payment from {:?} to {}",
            &quote.request_lookup_id,
            payment_lookup_id
        );

        tx.update_melt_quote_request_lookup_id(&mut quote, payment_lookup_id)
            .await?;
    }

    let mut proofs = tx.get_proofs(input_ys).await?;

    Mint::update_proofs_state(tx, &mut proofs, State::Spent).await?;

    // Clone the proofs out of the Acquired wrapper so that no database
    // row locks are held after this function returns.
    Ok((proofs.to_vec(), quote.inner()))
}

/// High-level melt finalization that handles the complete workflow.
///
/// This is the **single finalization path** for standalone melt operations —
/// both the normal saga flow and all recovery/async paths. Federated melts use
/// [`finalize_federation_melt_quote`], which does the same work in one
/// transaction. It orchestrates:
/// 1. Getting melt request info and input proof Y values
/// 2. Core finalization (mark proofs spent, update quote to Paid)
/// 3. Processing change (if needed)
/// 4. Recording the completed operation (fee tracking, audit)
/// 5. Deleting the saga record
/// 6. Transaction commit and pubsub notification
///
/// # Arguments
///
/// * `mint` - Mint instance
/// * `db` - Database connection
/// * `pubsub` - Pubsub manager
/// * `quote` - Melt quote to finalize
/// * `total_spent` - Amount spent on payment
/// * `payment_proof` - Payment preimage (if any)
/// * `payment_lookup_id` - Payment lookup identifier
/// * `operation_id` - Saga operation ID for recording the completed operation
///   and deleting the saga. When `None`, operation recording and saga deletion
///   are skipped (should not happen in practice).
///
/// # Returns
///
/// `Option<Vec<BlindSignature>>` - Change signatures (if any)
#[allow(clippy::too_many_arguments)]
pub async fn finalize_melt_quote(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    operation_id: Option<uuid::Uuid>,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    finalize_melt_quote_inner(
        mint,
        db,
        pubsub,
        quote,
        total_spent,
        payment_proof,
        payment_lookup_id,
        operation_id,
    )
    .await
}

/// Finalizes a paid federated melt in one database transaction.
///
/// Begins a transaction, runs [`finalize_federation_melt_in_transaction`],
/// commits, and only then publishes. Every federated finalization caller uses
/// this function or the in-transaction function directly, so the quote, the
/// inputs, the change shares, the completed operation and the saga cleanup
/// either all commit or none do.
///
/// # Returns
///
/// The change signatures, if any. When an earlier run already finalized the
/// melt, the stored change signatures are returned.
#[cfg(feature = "federation")]
#[allow(clippy::too_many_arguments)]
pub async fn finalize_federation_melt_quote(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    operation_id: Option<uuid::Uuid>,
    federation_operation_id: FederationOperationId,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    // Load the config before the transaction opens: rebuilding it reads the
    // database, which must not happen while this transaction holds the
    // connection.
    let config = mint
        .effective_federation_config()
        .await?
        .ok_or(Error::Internal)?;

    let mut tx = db.begin_transaction().await?;
    let outcome = finalize_federation_melt_in_transaction(
        &mut tx,
        &config,
        quote,
        total_spent,
        payment_proof,
        payment_lookup_id,
        operation_id,
        federation_operation_id,
    )
    .await;
    let finalization = match outcome {
        Ok(finalization) => finalization,
        Err(err) => {
            tx.rollback().await?;
            return Err(err);
        }
    };
    tx.commit().await?;

    if finalization.is_already_completed() {
        let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
        return Ok(if sigs.is_empty() { None } else { Some(sigs) });
    }

    let change_sigs = finalization.change_signatures().map(<[_]>::to_vec);
    finalization.publish(pubsub);
    Ok(change_sigs)
}

/// What a federated melt finalization publishes after its transaction commits.
///
/// Returned by [`finalize_federation_melt_in_transaction`]. It carries the
/// proof Ys this finalization moved to `Spent`, the paid quote, its payment
/// proof and the change signatures.
#[cfg(feature = "federation")]
#[derive(Debug)]
#[must_use = "a federated melt finalization must be published after the transaction commits"]
pub(crate) struct FederationMeltFinalization {
    spent_ys: Vec<PublicKey>,
    /// `None` when an earlier run already completed the finalization.
    finalized: Option<FinalizedFederationMelt>,
}

#[cfg(feature = "federation")]
#[derive(Debug)]
struct FinalizedFederationMelt {
    quote: MeltQuote,
    payment_proof: Option<String>,
    change_sigs: Option<Vec<BlindSignature>>,
    #[cfg(feature = "prometheus")]
    confirmed_total_spent: Option<Amount<CurrencyUnit>>,
}

#[cfg(feature = "federation")]
impl FederationMeltFinalization {
    fn already_completed() -> Self {
        Self {
            spent_ys: Vec::new(),
            finalized: None,
        }
    }

    /// Whether an earlier run had already finalized the melt.
    pub(crate) fn is_already_completed(&self) -> bool {
        self.finalized.is_none()
    }

    /// Change signatures created by this finalization.
    pub(crate) fn change_signatures(&self) -> Option<&[BlindSignature]> {
        self.finalized
            .as_ref()
            .and_then(|finalized| finalized.change_sigs.as_deref())
    }

    /// Convert into notifications that can be merged with other melt changes.
    pub(crate) fn into_notifications(self) -> MeltNotifications {
        let mut notifications = MeltNotifications::default();
        notifications
            .proof_states
            .extend(self.spent_ys.into_iter().map(|y| (y, State::Spent)));
        if let Some(finalized) = self.finalized {
            #[cfg(feature = "prometheus")]
            if let Some(total_spent) = finalized.confirmed_total_spent {
                notifications
                    .confirmed_payments
                    .push((finalized.quote.clone(), total_spent));
            }
            notifications
                .quote_statuses
                .push(MeltQuoteStatusNotification {
                    quote: finalized.quote,
                    payment_proof: finalized.payment_proof,
                    change: finalized.change_sigs,
                    state: MeltQuoteState::Paid,
                });
        }
        notifications
    }

    /// Publish the spent proofs and the paid quote. Call only after commit.
    pub(crate) fn publish(self, pubsub: &PubSubManager) {
        self.into_notifications().publish(pubsub);
    }
}

/// Finalizes a paid federated melt inside a caller-supplied transaction.
///
/// This is the single federated finalization implementation. Within `tx` it:
/// 1. Locks the quote and loads the melt request
/// 2. Validates amounts like [`finalize_melt_core`]
/// 3. Sets the quote `Paid` and spends the inputs
/// 4. Computes the local change shares and stores shares and signatures
/// 5. Records the completed operation with its fee breakdown
/// 6. Deletes the melt request and the saga
///
/// It never commits, rolls back or publishes. The caller commits and then
/// publishes the returned [`FederationMeltFinalization`]. If the caller rolls
/// back, nothing of the finalization remains.
///
/// Idempotent branches: a quote already `Paid` with the same settlement
/// continues with change and cleanup, and a missing melt request or missing
/// inputs mean an earlier run completed, which only deletes the saga.
///
/// `config` must be loaded before `tx` was opened.
#[cfg(feature = "federation")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_federation_melt_in_transaction(
    tx: &mut DynMintTransaction,
    config: &FederationRuntimeConfig,
    quote: &MeltQuote,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    operation_id: Option<uuid::Uuid>,
    federation_operation_id: FederationOperationId,
) -> Result<FederationMeltFinalization, Error> {
    tracing::debug!("Finalizing federated melt quote {}", quote.id);

    let total_spent = total_spent_for_quote_unit(&total_spent, &quote.unit)?;

    let settlement_matches = |stored_quote: &MeltQuote| {
        stored_quote.request_lookup_id.as_ref() == Some(payment_lookup_id)
            && stored_quote.payment_proof == payment_proof
    };

    let locked_quote = load_melt_quotes_exclusively(tx, &quote.id).await?;

    let Some(melt_request_info) = tx.get_melt_request_and_blinded_messages(&quote.id).await? else {
        if locked_quote.state == MeltQuoteState::Paid && !settlement_matches(&locked_quote) {
            return Err(Error::PaidQuote);
        }

        tracing::warn!(
            "No melt request found for quote {} - may have been completed already",
            quote.id
        );
        // Melt request already cleaned up (completed in a prior run). Delete
        // the saga if present so recovery doesn't retry.
        if let Some(op_id) = operation_id {
            tx.delete_saga(&op_id).await?;
        }
        return Ok(FederationMeltFinalization::already_completed());
    };

    let input_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;
    if input_ys.is_empty() {
        tracing::warn!(
            "No input proofs found for quote {} - may have been completed already",
            quote.id
        );
        if let Some(op_id) = operation_id {
            tx.delete_saga(&op_id).await?;
        }
        return Ok(FederationMeltFinalization::already_completed());
    }

    #[cfg(feature = "prometheus")]
    let should_record_payment_metrics = locked_quote.state != MeltQuoteState::Paid;

    // A quote already Paid with the same settlement only needs its remaining
    // Pending inputs spent before change and cleanup.
    let (proofs, finalized_quote, spent_ys) = if locked_quote.state == MeltQuoteState::Paid {
        let locked_quote = locked_quote.inner();
        if !settlement_matches(&locked_quote) {
            return Err(Error::PaidQuote);
        }

        tracing::info!(
            "Melt quote {} already Paid, skipping to change/cleanup",
            quote.id
        );
        let mut proofs_with_state = tx.get_proofs(&input_ys).await?;
        let spend_pending = proofs_with_state.state == State::Pending;
        if spend_pending {
            Mint::update_proofs_state(tx, &mut proofs_with_state, State::Spent).await?;
        }
        let spent_ys = if spend_pending {
            input_ys.clone()
        } else {
            Vec::new()
        };
        (proofs_with_state.to_vec(), locked_quote, spent_ys)
    } else {
        let (proofs, finalized_quote) = finalize_melt_core_in_transaction(
            tx,
            locked_quote,
            &input_ys,
            melt_request_info.inputs_amount.clone(),
            melt_request_info.inputs_fee.clone(),
            total_spent.clone(),
            payment_proof.clone(),
            payment_lookup_id,
        )
        .await?;
        (proofs, finalized_quote, input_ys.clone())
    };

    let fee_breakdown = match operation_id {
        Some(_) => Some(crate::mint::verification::calculate_proofs_fee(
            &config.keysets(),
            &proofs,
        )?),
        None => None,
    };

    let signature_share_provenance =
        federation_signature_share_provenance_in_transaction(tx, federation_operation_id).await?;
    let change_sigs = add_federation_melt_change(
        tx,
        config,
        &finalized_quote.id,
        federation_operation_id,
        signature_share_provenance,
        FederationMeltChange {
            inputs_amount: melt_request_info.inputs_amount.clone(),
            total_spent: total_spent.clone(),
            inputs_fee: melt_request_info.inputs_fee.clone(),
            outputs: melt_request_info.change_outputs.clone(),
        },
    )
    .await?;

    tx.delete_melt_request(&finalized_quote.id).await?;

    if let (Some(op_id), Some(fee_breakdown)) = (operation_id, fee_breakdown.as_ref()) {
        let change_amount = match change_sigs.as_ref() {
            Some(sigs) => Amount::try_sum(sigs.iter().map(|sig| sig.amount))?,
            None => Amount::ZERO,
        };

        // An accepted federation operation provides order but no wall-clock
        // time, so completion time is recorded as unknown (zero).
        let mut operation = mint_types::Operation::new(
            op_id,
            mint_types::OperationKind::Melt,
            Amount::ZERO,
            melt_request_info.inputs_amount.clone().into(),
            fee_breakdown.total,
            Some(0),
            Some(finalized_quote.payment_method.clone()),
        );
        operation.add_change(change_amount);

        let payment_fee = total_spent.checked_sub(&finalized_quote.amount())?;
        operation.set_payment_details(finalized_quote.amount().into(), payment_fee.into());

        tx.add_completed_operation(&operation, &fee_breakdown.per_keyset)
            .await?;
    }

    if let Some(op_id) = operation_id {
        tx.delete_saga(&op_id).await?;
    }

    #[cfg(test)]
    if crate::test_helpers::mint::should_fail_for("FEDERATION_MELT_FINALIZE_BEFORE_COMMIT") {
        return Err(Error::Internal);
    }

    tracing::debug!(
        quote_id = %finalized_quote.id,
        saga_id = ?operation_id,
        payment_lookup_id = %payment_lookup_id,
        quote_amount = %finalized_quote.amount(),
        fee_reserve = %finalized_quote.fee_reserve(),
        input_fee = %melt_request_info.inputs_fee,
        inputs_amount = %melt_request_info.inputs_amount,
        total_spent = %total_spent,
        proof_count = input_ys.len(),
        requested_change_output_count = melt_request_info.change_outputs.len(),
        change_signature_count = change_sigs.as_ref().map_or(0, Vec::len),
        "federated melt finalization staged; it takes effect when the transaction commits",
    );

    Ok(FederationMeltFinalization {
        spent_ys,
        finalized: Some(FinalizedFederationMelt {
            quote: finalized_quote,
            payment_proof,
            change_sigs,
            #[cfg(feature = "prometheus")]
            confirmed_total_spent: should_record_payment_metrics.then_some(total_spent),
        }),
    })
}

#[cfg(feature = "federation")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_federation_internal_melt_quote(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    operation_id: uuid::Uuid,
    federation_operation_id: FederationOperationId,
) -> Result<FederationInternalMeltSettlement, Error> {
    let request = quote.request.to_string();

    let mut read_tx = db.begin_transaction().await?;
    let Some(mint_quote) = read_tx.get_mint_quote_by_request(&request).await? else {
        read_tx.rollback().await?;
        return Ok(FederationInternalMeltSettlement::NotInternal);
    };
    if mint_quote.unit != quote.unit {
        read_tx.rollback().await?;
        return Ok(FederationInternalMeltSettlement::NotInternal);
    }
    let Some(melt_request_info) = read_tx
        .get_melt_request_and_blinded_messages(&quote.id)
        .await?
    else {
        read_tx.rollback().await?;
        return Err(Error::Internal);
    };
    read_tx.rollback().await?;

    let total_spent = quote.amount();
    let signature_share_provenance =
        federation_signature_share_provenance_for_operation(db, federation_operation_id).await?;
    let config = mint
        .effective_federation_config()
        .await?
        .ok_or(Error::Internal)?;
    let prepared_change = prepare_federation_melt_change(
        &config,
        &quote.id,
        federation_operation_id,
        signature_share_provenance,
        FederationMeltChange {
            inputs_amount: melt_request_info.inputs_amount.clone(),
            total_spent: total_spent.clone(),
            inputs_fee: melt_request_info.inputs_fee.clone(),
            outputs: melt_request_info.change_outputs.clone(),
        },
    )?;

    let mut tx = db.begin_transaction().await?;
    let mut mint_quote = match tx.get_mint_quote_by_request(&request).await? {
        Some(mint_quote) if mint_quote.unit == quote.unit => mint_quote,
        _ => {
            tx.rollback().await?;
            return Ok(FederationInternalMeltSettlement::NotInternal);
        }
    };

    if (mint_quote.state() == MintQuoteState::Issued || mint_quote.state() == MintQuoteState::Paid)
        && mint_quote.payment_method
            == crate::mint::PaymentMethod::Known(cdk_common::nut00::KnownMethod::Bolt11)
    {
        tx.rollback().await?;
        return Err(Error::RequestAlreadyPaid);
    }

    let melt_request_info = match tx.get_melt_request_and_blinded_messages(&quote.id).await? {
        Some(info) => info,
        None => {
            tx.rollback().await?;
            return Err(Error::Internal);
        }
    };

    if let Some(ref amount) = mint_quote.amount {
        if amount > &melt_request_info.inputs_amount {
            tx.rollback().await?;
            return Err(Error::InsufficientFunds);
        }
    }

    let mut locked_quote = load_melt_quotes_exclusively(&mut tx, &quote.id).await?;
    if locked_quote.state == MeltQuoteState::Paid {
        tx.rollback().await?;
        return Err(Error::PaidQuote);
    }

    let input_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;
    if input_ys.is_empty() {
        tx.rollback().await?;
        return Err(Error::Internal);
    }

    let net_inputs = match melt_request_info
        .inputs_amount
        .checked_sub(&melt_request_info.inputs_fee)
    {
        Ok(net_inputs) => net_inputs,
        Err(err) => {
            tx.rollback().await?;
            return Err(err.into());
        }
    };
    let total_spent = match total_spent.convert_to(net_inputs.unit()) {
        Ok(total_spent) => total_spent,
        Err(err) => {
            tx.rollback().await?;
            return Err(err.into());
        }
    };

    if net_inputs < total_spent {
        tracing::error!(
            "Over paid internal melt quote {}: net_inputs ({}) < total_spent ({}).",
            quote.id,
            net_inputs.display_with_unit(),
            total_spent.display_with_unit(),
        );
    }

    let payment_lookup_id = quote
        .request_lookup_id
        .clone()
        .unwrap_or_else(|| cdk_common::payment::PaymentIdentifier::CustomId(quote.id.to_string()));

    // The accepted operation orders this payment but carries no wall-clock
    // evidence. Zero records an unknown time without confusing journal order
    // with a Unix timestamp.
    if let Err(err) = mint_quote.add_payment(total_spent.clone(), quote.id.to_string(), Some(0)) {
        tx.rollback().await?;
        return Err(err);
    }
    tx.update_mint_quote(&mut mint_quote).await?;

    tx.update_melt_quote_state(&mut locked_quote, MeltQuoteState::Paid, None)
        .await?;
    locked_quote.state = MeltQuoteState::Paid;

    if locked_quote.request_lookup_id.as_ref() != Some(&payment_lookup_id) {
        tx.update_melt_quote_request_lookup_id(&mut locked_quote, &payment_lookup_id)
            .await?;
    }

    let mut proofs = tx.get_proofs(&input_ys).await?;
    if let Err(err) = Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent).await {
        tx.rollback().await?;
        return Err(err);
    }

    if !prepared_change.signature_share_records.is_empty() {
        tx.add_federation_signature_shares(&prepared_change.signature_share_records)
            .await?;
    }

    if let Some(change_sigs) = prepared_change.change_sigs.as_ref() {
        tx.add_blind_signatures(
            &prepared_change.blinded_secrets,
            change_sigs,
            Some(quote.id.clone()),
        )
        .await?;
    }

    let fee_breakdown =
        crate::mint::verification::calculate_proofs_fee(&config.keysets(), &proofs)?;
    let change_amount = match prepared_change.change_sigs.as_ref() {
        Some(sigs) => match Amount::try_sum(sigs.iter().map(|sig| sig.amount)) {
            Ok(amount) => amount,
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        },
        None => Amount::default(),
    };

    tx.delete_melt_request(&quote.id).await?;

    // Consensus orders completion but does not attest its wall-clock time.
    let mut operation = mint_types::Operation::new(
        operation_id,
        mint_types::OperationKind::Melt,
        Amount::ZERO,
        melt_request_info.inputs_amount.clone().into(),
        fee_breakdown.total,
        Some(0),
        Some(locked_quote.payment_method.clone()),
    );
    operation.add_change(change_amount);
    operation.set_payment_details(locked_quote.amount().into(), Amount::ZERO);
    tx.add_completed_operation(&operation, &fee_breakdown.per_keyset)
        .await?;
    tx.delete_saga(&operation_id).await?;

    let paid_mint_quote = mint_quote.inner();
    let paid_melt_quote = locked_quote.inner();
    tx.commit().await?;

    for pk in input_ys.iter() {
        pubsub.proof_state((*pk, State::Spent));
    }
    pubsub.mint_quote_payment(&paid_mint_quote, paid_mint_quote.amount_paid());
    pubsub.melt_quote_status(
        &paid_melt_quote,
        None,
        prepared_change.change_sigs.clone(),
        MeltQuoteState::Paid,
    );

    #[cfg(feature = "prometheus")]
    record_confirmed_payment_metrics(&paid_melt_quote, &total_spent);

    Ok(FederationInternalMeltSettlement::Finalized {
        quote: Box::new(paid_melt_quote),
        change_sigs: prepared_change.change_sigs,
    })
}

#[allow(clippy::too_many_arguments)]
async fn finalize_melt_quote_inner(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    total_spent: Amount<CurrencyUnit>,
    payment_proof: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    operation_id: Option<uuid::Uuid>,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    tracing::debug!("Finalizing melt quote {}", quote.id);

    let total_spent = total_spent_for_quote_unit(&total_spent, &quote.unit)?;

    let settlement_matches = |stored_quote: &MeltQuote| {
        stored_quote.request_lookup_id.as_ref() == Some(payment_lookup_id)
            && stored_quote.payment_proof == payment_proof
    };

    let mut tx = db.begin_transaction().await?;

    // Acquire lock on the quote for safe state update

    let locked_quote = load_melt_quotes_exclusively(&mut tx, &quote.id).await?;

    // Get melt request info
    let melt_request_info = match tx.get_melt_request_and_blinded_messages(&quote.id).await? {
        Some(info) => info,
        None => {
            if locked_quote.state == MeltQuoteState::Paid {
                let locked_quote = locked_quote.inner();

                if locked_quote.request_lookup_id.as_ref() != Some(payment_lookup_id)
                    || locked_quote.payment_proof != payment_proof
                {
                    tx.rollback().await?;
                    return Err(Error::PaidQuote);
                }
            }

            tracing::warn!(
                "No melt request found for quote {} - may have been completed already",
                quote.id
            );
            // Melt request already cleaned up (likely completed in a prior run).
            // Delete the saga if present so recovery doesn't retry.
            if let Some(op_id) = operation_id {
                if let Err(e) = tx.delete_saga(&op_id).await {
                    tracing::warn!("Failed to delete saga {} during early return: {}", op_id, e);
                }
                tx.commit().await?;
            } else {
                tx.rollback().await?;
            }

            let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
            return Ok(if sigs.is_empty() { None } else { Some(sigs) });
        }
    };

    // Get input proof Y values
    let input_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;

    if input_ys.is_empty() {
        tracing::warn!(
            "No input proofs found for quote {} - may have been completed already",
            quote.id
        );
        // No proofs (likely completed in a prior run).
        // Delete the saga if present so recovery doesn't retry.
        if let Some(op_id) = operation_id {
            if let Err(e) = tx.delete_saga(&op_id).await {
                tracing::warn!("Failed to delete saga {} during early return: {}", op_id, e);
            }
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }

        let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
        return Ok(if sigs.is_empty() { None } else { Some(sigs) });
    }

    #[cfg(feature = "prometheus")]
    let should_record_payment_metrics = locked_quote.state != MeltQuoteState::Paid;

    // Check if TX1 already completed (e.g., crash between TX1 commit and TX2 commit).
    // If the quote is already Paid, calling finalize_melt_core would fail on the
    // Paid→Paid state transition. Skip directly to change signing and cleanup so
    // the user receives their change.
    //
    // The proofs may still be Pending here; spend any that are still Pending,
    // otherwise this is a no-op.
    //
    // We still need the proofs for fee calculation (operation recording), so fetch
    // them from the DB even in the already-Paid case.
    let (proofs, quote) = if locked_quote.state == MeltQuoteState::Paid {
        let locked_quote = locked_quote.inner();

        if !settlement_matches(&locked_quote) {
            tx.rollback().await?;
            return Err(Error::PaidQuote);
        }

        tracing::info!(
            "Melt quote {} already Paid, skipping to change/cleanup",
            quote.id
        );
        let mut proofs_with_state = tx.get_proofs(&input_ys).await?;
        let spend_pending = proofs_with_state.state == State::Pending;
        if spend_pending {
            if let Err(err) =
                Mint::update_proofs_state(&mut tx, &mut proofs_with_state, State::Spent).await
            {
                tx.rollback().await?;
                return Err(err);
            }
        }
        let proofs = proofs_with_state.to_vec();
        tx.commit().await?;
        if spend_pending {
            for pk in input_ys.iter() {
                pubsub.proof_state((*pk, State::Spent));
            }
        }
        (proofs, locked_quote)
    } else {
        let (proofs, quote) = finalize_melt_core(
            tx,
            pubsub,
            locked_quote,
            &input_ys,
            melt_request_info.inputs_amount.clone(),
            melt_request_info.inputs_fee.clone(),
            total_spent.clone(),
            payment_proof.clone(),
            payment_lookup_id,
        )
        .await?;

        (proofs, quote)
    };

    // Process change (if needed) - opens new transaction.
    let change_result = process_melt_change(
        mint,
        db,
        &quote.id,
        melt_request_info.inputs_amount.clone(),
        total_spent.clone(),
        melt_request_info.inputs_fee.clone(),
        melt_request_info.change_outputs.clone(),
    )
    .await?;

    let (change_sigs, mut tx) = match change_result {
        MeltChangeResult::Ready { change_sigs, tx } => (change_sigs, tx),
        MeltChangeResult::AlreadyCompleted => {
            let stored_quote = db
                .get_melt_quote(&quote.id)
                .await?
                .ok_or(Error::UnknownQuote)?;

            if stored_quote.state != MeltQuoteState::Paid || !settlement_matches(&stored_quote) {
                return Err(Error::PaidQuote);
            }

            let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
            return Ok(if sigs.is_empty() { None } else { Some(sigs) });
        }
    };

    let change_amount_for_log = change_sigs
        .as_ref()
        .and_then(|sigs| Amount::try_sum(sigs.iter().map(|s| s.amount)).ok());
    let payment_fee_for_log = total_spent.checked_sub(&quote.amount()).ok();

    // Compute the fee breakdown from the spent proofs before cleanup.
    // We reuse the cloned proofs from TX1 / recovery so TX2 can atomically
    // persist the completed operation with the rest of the post-payment work.
    let fee_breakdown = if operation_id.is_some() {
        Some(match mint.get_proofs_fee(&proofs).await {
            Ok(fee_breakdown) => fee_breakdown,
            Err(err) => {
                tx.rollback().await?;
                return Err(err);
            }
        })
    } else {
        None
    };

    // Delete melt request tracking, completed operation, and saga in the same transaction.
    if let Err(err) = tx.delete_melt_request(&quote.id).await {
        tx.rollback().await?;
        return Err(err.into());
    }

    if let (Some(op_id), Some(fee_breakdown)) = (operation_id, fee_breakdown.as_ref()) {
        let change_amount = change_sigs
            .as_ref()
            .map(|sigs| {
                Amount::try_sum(sigs.iter().map(|s| s.amount))
                    .expect("Change amount cannot overflow")
            })
            .unwrap_or_default();

        let mut operation = mint_types::Operation::new(
            op_id,
            mint_types::OperationKind::Melt,
            Amount::ZERO,
            melt_request_info.inputs_amount.clone().into(),
            fee_breakdown.total,
            None,
            Some(quote.payment_method.clone()),
        );

        operation.add_change(change_amount);

        let payment_fee = match total_spent.checked_sub(&quote.amount()) {
            Ok(payment_fee) => payment_fee,
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        };
        operation.set_payment_details(quote.amount().into(), payment_fee.into());

        if let Err(err) = tx
            .add_completed_operation(&operation, &fee_breakdown.per_keyset)
            .await
        {
            tx.rollback().await?;
            return Err(err.into());
        }
    }

    if let Some(op_id) = operation_id {
        if let Err(err) = tx.delete_saga(&op_id).await {
            tx.rollback().await?;
            return Err(err.into());
        }
    }

    // Commit TX2 (change signatures + operation record + melt request + saga cleanup)
    tx.commit().await?;

    // Publish quote status change
    pubsub.melt_quote_status(
        &quote,
        payment_proof,
        change_sigs.clone(),
        MeltQuoteState::Paid,
    );

    tracing::info!(
        quote_id = %quote.id,
        saga_id = ?operation_id,
        payment_lookup_id = %payment_lookup_id,
        new_quote_state = %MeltQuoteState::Paid,
        quote_amount = %quote.amount(),
        fee_reserve = %quote.fee_reserve(),
        payment_fee = ?payment_fee_for_log,
        input_fee = %melt_request_info.inputs_fee,
        inputs_amount = %melt_request_info.inputs_amount,
        total_spent = %total_spent,
        change_amount = ?change_amount_for_log,
        proof_count = input_ys.len(),
        requested_change_output_count = melt_request_info.change_outputs.len(),
        change_signature_count = change_sigs.as_ref().map_or(0, Vec::len),
        "melt finalized; quote is paid and input proofs are spent",
    );

    #[cfg(feature = "prometheus")]
    if should_record_payment_metrics {
        record_confirmed_payment_metrics(&quote, &total_spent);
    }

    Ok(change_sigs)
}
