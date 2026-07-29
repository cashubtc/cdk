//! Shared logic for melt operations across saga and startup check.
//!
//! This module contains common functions used by both:
//! - `melt_saga`: Normal melt operation flow
//! - `start_up_check`: Recovery of interrupted melts during startup
//!
//! The functions here ensure consistency between these two code paths.

use cdk_common::amount::MSAT_IN_SAT;
use cdk_common::database::mint::Acquired;
use cdk_common::database::{self, DynMintDatabase, DynMintTransaction};
#[cfg(feature = "federation")]
use cdk_common::federation::{
    FederationAcceptedSigningRequest, FederationJournalEntry, FederationOperationId,
    FederationSignatureShareRecord,
};
use cdk_common::mint::{self as mint_types};
#[cfg(feature = "federation")]
use cdk_common::nuts::MintQuoteState;
use cdk_common::nuts::{BlindSignature, BlindedMessage, MeltQuoteState, Proofs, State};
use cdk_common::{Amount, CurrencyUnit, Error, PublicKey, QuoteId};
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use cdk_signatory::signatory::SignatoryKeySet;

use crate::mint::subscription::PubSubManager;
use crate::mint::MeltQuote;
use crate::Mint;

/// Retrieves fee and amount configuration for the keyset matching the change outputs.
///
/// Searches active keysets for one matching the first output's keyset_id.
/// Used during change calculation for melts.
///
/// # Arguments
///
/// * `keysets` - Arc reference to the loaded keysets
/// * `outputs` - Change output blinded messages
///
/// # Returns
///
/// Fee per thousand and allowed amounts for the keyset, or default if not found
pub fn get_keyset_fee_and_amounts(
    keysets: &arc_swap::ArcSwap<Vec<SignatoryKeySet>>,
    outputs: &[BlindedMessage],
) -> cdk_common::amount::FeeAndAmounts {
    keysets
        .load()
        .iter()
        .filter_map(|keyset| {
            if keyset.active && Some(keyset.id) == outputs.first().map(|x| x.keyset_id) {
                Some((keyset.input_fee_ppk, keyset.amounts.clone()).into())
            } else {
                None
            }
        })
        .next()
        .unwrap_or_else(|| (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into())
}

#[cfg(feature = "federation")]
fn get_federation_keyset_fee_and_amounts(
    config: &crate::mint::FederationRuntimeConfig,
    outputs: &[BlindedMessage],
) -> cdk_common::amount::FeeAndAmounts {
    config
        .public_config()
        .keysets
        .iter()
        .filter_map(|keyset| {
            if keyset.active && Some(keyset.id) == outputs.first().map(|output| output.keyset_id) {
                Some(
                    (
                        keyset.input_fee_ppk,
                        keyset
                            .aggregate_keys
                            .keys()
                            .map(|amount| amount.to_u64())
                            .collect::<Vec<_>>(),
                    )
                        .into(),
                )
            } else {
                None
            }
        })
        .next()
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
    match (total_spent.unit(), quote_unit) {
        (spent_unit, quote_unit) if spent_unit == quote_unit => Ok(total_spent.clone()),
        (CurrencyUnit::Msat, CurrencyUnit::Sat) => {
            let rounded_sats = total_spent.value().div_ceil(MSAT_IN_SAT);
            Ok(Amount::new(rounded_sats, CurrencyUnit::Sat))
        }
        _ => total_spent.convert_to(quote_unit).map_err(Error::from),
    }
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
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        tx.rollback().await?;
        return Ok(());
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
            tx.rollback().await?;
            return Ok(());
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
                    tx.rollback().await?;
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
                tx.rollback().await?;
                return Err(Error::UnknownPaymentState);
            }
        }
    }

    let quote_option = if let Some(mut quote) = locked_quotes.target {
        match quote.state {
            MeltQuoteState::Pending => {
                tx.update_melt_quote_state(&mut quote, MeltQuoteState::Unpaid, None)
                    .await?;
                Some(quote)
            }
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                // Already in a non-pending state; fall through to saga / melt-request
                // cleanup so rollback remains idempotent.
                None
            }
            MeltQuoteState::Paid => {
                tx.rollback().await?;
                return Err(Error::PaidQuote);
            }
            state => {
                tracing::warn!(
                    "Refusing rollback for melt quote {} in unexpected state {}",
                    quote_id,
                    state
                );
                tx.rollback().await?;
                return Err(Error::UnknownPaymentState);
            }
        }
    } else {
        None
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

    tx.commit().await?;

    // Publish proof state changes
    if proofs_recovered {
        for pk in input_ys.iter() {
            pubsub.proof_state((*pk, State::Unspent));
        }
    }

    if let Some(quote) = quote_option {
        pubsub.melt_quote_status(&quote, None, None, MeltQuoteState::Unpaid);
    }

    tracing::info!(
        "Successfully rolled back melt quote {} and deleted saga {}",
        quote_id,
        operation_id
    );

    Ok(())
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
        Some(_) => {
            return begin_melt_change_without_signatures(db, quote_id).await;
        }
        None => {
            tracing::warn!(
                "Fee was too high for quote {}. inputs_amount: {}, total_spent: {}, inputs_fee: {}",
                quote_id,
                inputs_amount,
                total_spent,
                inputs_fee
            );
            return begin_melt_change_without_signatures(db, quote_id).await;
        }
    };

    if change_outputs.is_empty() {
        return begin_melt_change_without_signatures(db, quote_id).await;
    }

    // Get keyset configuration
    let fee_and_amounts = get_keyset_fee_and_amounts(&mint.keysets, &change_outputs);

    // Split change into denominations
    let mut amounts: Vec<Amount> = change_target.split(&fee_and_amounts)?;

    if change_outputs.len() < amounts.len() {
        tracing::debug!(
            "Providing change requires {} blinded messages, but only {} provided",
            amounts.len(),
            change_outputs.len()
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
    let change_sigs = mint.blind_sign(blinded_messages_to_sign.clone()).await?;

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
        .get_federation_journal_entry_by_operation_id(federation_operation_id)
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
        quote: MeltQuote,
        change_sigs: Option<Vec<BlindSignature>>,
    },
}

#[cfg(feature = "federation")]
#[allow(clippy::too_many_arguments)]
async fn prepare_federation_melt_change(
    mint: &super::super::Mint,
    quote_id: &QuoteId,
    federation_operation_id: FederationOperationId,
    signature_share_provenance: FederationSignatureShareProvenance,
    inputs_amount: Amount<CurrencyUnit>,
    total_spent: Amount<CurrencyUnit>,
    inputs_fee: Amount<CurrencyUnit>,
    change_outputs: Vec<BlindedMessage>,
) -> Result<PreparedFederationMeltChange, Error> {
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

    let config = mint
        .effective_federation_config()
        .await?
        .ok_or(Error::Internal)?;
    let fee_and_amounts = get_federation_keyset_fee_and_amounts(&config, &change_outputs);
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

    #[cfg(feature = "prometheus")]
    let started = std::time::Instant::now();
    let signing_request = FederationAcceptedSigningRequest::from_journal_entry(
        config.public_config(),
        &signature_share_provenance.entry,
        &blinded_messages_to_sign,
    )?;
    let share_set = config
        .private_config()
        .sign_accepted_blinded_messages(config.public_config(), &signing_request);
    #[cfg(feature = "prometheus")]
    METRICS.record_federation_signing_share_latency(
        &config.member_id().to_string(),
        "melt",
        started.elapsed().as_secs_f64(),
    );
    let share_set = share_set?;
    let signature_share_records = FederationSignatureShareRecord::from_accepted_journal_entry(
        &share_set,
        &signature_share_provenance.entry,
    )?;
    let change_sigs = share_set
        .shares
        .into_iter()
        .map(|share| BlindSignature {
            amount: share.amount,
            keyset_id: share.keyset_id,
            c: PublicKey::from(share.share.signature()),
            dleq: None,
        })
        .collect::<Vec<_>>();
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

#[cfg(feature = "federation")]
pub async fn process_federation_melt_change(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    quote_id: &QuoteId,
    federation_operation_id: FederationOperationId,
    signature_share_provenance: FederationSignatureShareProvenance,
    inputs_amount: Amount<CurrencyUnit>,
    total_spent: Amount<CurrencyUnit>,
    inputs_fee: Amount<CurrencyUnit>,
    change_outputs: Vec<BlindedMessage>,
) -> Result<
    (
        Option<Vec<BlindSignature>>,
        Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    ),
    Error,
> {
    let prepared_change = prepare_federation_melt_change(
        mint,
        quote_id,
        federation_operation_id,
        signature_share_provenance,
        inputs_amount,
        total_spent,
        inputs_fee,
        change_outputs,
    )
    .await?;
    let mut tx = db.begin_transaction().await?;

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

    Ok((prepared_change.change_sigs, tx))
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
        tx.rollback().await?;
        return Err(Error::IncorrectQuoteAmount);
    }

    // Validate inputs amount
    let net_inputs = match inputs_amount.checked_sub(&inputs_fee) {
        Ok(net_inputs) => net_inputs,
        Err(err) => {
            tx.rollback().await?;
            return Err(err.into());
        }
    };

    // Convert total_spent to the same unit as net_inputs for comparison.
    // Backends should return total_spent in the quote's unit, but we convert defensively.
    let total_spent = match total_spent.convert_to(net_inputs.unit()) {
        Ok(total_spent) => total_spent,
        Err(err) => {
            tx.rollback().await?;
            return Err(err.into());
        }
    };

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
    if let Err(err) = tx
        .update_melt_quote_state(&mut quote, MeltQuoteState::Paid, payment_proof.clone())
        .await
    {
        tx.rollback().await?;
        return Err(err.into());
    }

    quote.state = MeltQuoteState::Paid;

    // Update payment lookup ID if changed
    if quote.request_lookup_id.as_ref() != Some(payment_lookup_id) {
        tracing::info!(
            "Payment lookup id changed post payment from {:?} to {}",
            &quote.request_lookup_id,
            payment_lookup_id
        );

        if let Err(err) = tx
            .update_melt_quote_request_lookup_id(&mut quote, payment_lookup_id)
            .await
        {
            tx.rollback().await?;
            return Err(err.into());
        }
    }

    let mut proofs = match tx.get_proofs(input_ys).await {
        Ok(proofs) => proofs,
        Err(err) => {
            tx.rollback().await?;
            return Err(err.into());
        }
    };

    if let Err(err) = Mint::update_proofs_state(&mut tx, &mut proofs, State::Spent).await {
        tx.rollback().await?;
        return Err(err);
    }

    tx.commit().await?;

    // Publish proof state changes
    for pk in input_ys.iter() {
        pubsub.proof_state((*pk, State::Spent));
    }

    // Clone the proofs out of the Acquired wrapper so that no database
    // row locks are held after this function returns.
    Ok((proofs.to_vec(), quote.inner()))
}

/// High-level melt finalization that handles the complete workflow.
///
/// This is the **single finalization path** for all melt operations — both the
/// normal saga flow and all recovery/async paths. It orchestrates:
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
        #[cfg(feature = "federation")]
        None,
    )
    .await
}

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
    finalize_melt_quote_inner(
        mint,
        db,
        pubsub,
        quote,
        total_spent,
        payment_proof,
        payment_lookup_id,
        operation_id,
        Some(federation_operation_id),
    )
    .await
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
    let prepared_change = prepare_federation_melt_change(
        mint,
        &quote.id,
        federation_operation_id,
        signature_share_provenance,
        melt_request_info.inputs_amount.clone(),
        total_spent.clone(),
        melt_request_info.inputs_fee.clone(),
        melt_request_info.change_outputs.clone(),
    )
    .await?;
    let config = mint
        .effective_federation_config()
        .await?
        .ok_or(Error::Internal)?;

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

    let fee_breakdown = mint.get_federation_proofs_fee_with_config(&config, &proofs)?;
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
        quote: paid_melt_quote,
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
    #[cfg(feature = "federation")] federation_operation_id: Option<FederationOperationId>,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    tracing::info!("Finalizing melt quote {}", quote.id);

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

    #[cfg(feature = "federation")]
    let federation_fee_breakdown = if operation_id.is_some() && federation_operation_id.is_some() {
        let config = mint
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        Some(mint.get_federation_proofs_fee_with_config(&config, &proofs)?)
    } else {
        None
    };

    #[cfg(feature = "federation")]
    let signature_share_provenance = match federation_operation_id {
        Some(federation_operation_id) => Some(
            federation_signature_share_provenance_for_operation(db, federation_operation_id)
                .await?,
        ),
        None => None,
    };

    #[cfg(feature = "federation")]
    // Preserve standalone completion time when this is not a federation melt.
    // An accepted federation operation provides order but no wall-clock time.
    let federation_completed_at = signature_share_provenance.as_ref().map(|_| 0);

    // Process change (if needed) - opens new transaction.
    #[cfg(feature = "federation")]
    let change_result = if let Some(federation_operation_id) = federation_operation_id {
        let signature_share_provenance = signature_share_provenance.ok_or(Error::Internal)?;
        let (change_sigs, tx) = process_federation_melt_change(
            mint,
            db,
            &quote.id,
            federation_operation_id,
            signature_share_provenance,
            melt_request_info.inputs_amount.clone(),
            total_spent.clone(),
            melt_request_info.inputs_fee.clone(),
            melt_request_info.change_outputs.clone(),
        )
        .await?;
        MeltChangeResult::Ready { change_sigs, tx }
    } else {
        process_melt_change(
            mint,
            db,
            &quote.id,
            melt_request_info.inputs_amount.clone(),
            total_spent.clone(),
            melt_request_info.inputs_fee.clone(),
            melt_request_info.change_outputs.clone(),
        )
        .await?
    };
    #[cfg(not(feature = "federation"))]
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

    // Compute the fee breakdown from the spent proofs before cleanup.
    // We reuse the cloned proofs from TX1 / recovery so TX2 can atomically
    // persist the completed operation with the rest of the post-payment work.
    let fee_breakdown = if operation_id.is_some() {
        #[cfg(feature = "federation")]
        {
            if let Some(fee_breakdown) = federation_fee_breakdown.as_ref() {
                Some(fee_breakdown.clone())
            } else {
                Some(match mint.get_proofs_fee(&proofs).await {
                    Ok(fee_breakdown) => fee_breakdown,
                    Err(err) => {
                        tx.rollback().await?;
                        return Err(err);
                    }
                })
            }
        }
        #[cfg(not(feature = "federation"))]
        {
            Some(match mint.get_proofs_fee(&proofs).await {
                Ok(fee_breakdown) => fee_breakdown,
                Err(err) => {
                    tx.rollback().await?;
                    return Err(err);
                }
            })
        }
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
            #[cfg(feature = "federation")]
            federation_completed_at,
            #[cfg(not(feature = "federation"))]
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

    #[cfg(all(test, feature = "federation"))]
    if federation_operation_id.is_some()
        && crate::test_helpers::mint::should_fail_for(
            "FEDERATION_MELT_FINALIZE_BEFORE_CLEANUP_COMMIT",
        )
    {
        return Err(Error::Internal);
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

    tracing::info!("Successfully finalized melt quote {}", quote.id);

    #[cfg(feature = "prometheus")]
    if should_record_payment_metrics {
        record_confirmed_payment_metrics(&quote, &total_spent);
    }

    Ok(change_sigs)
}
