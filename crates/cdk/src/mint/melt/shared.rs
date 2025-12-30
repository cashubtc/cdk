//! Shared logic for melt operations across saga and startup check.
//!
//! This module contains common functions used by both:
//! - `melt_saga`: Normal melt operation flow
//! - `start_up_check`: Recovery of interrupted melts during startup
//!
//! The functions here ensure consistency between these two code paths.

use cdk_common::database::{self, Acquired, DynMintDatabase};
use cdk_common::nuts::{BlindSignature, BlindedMessage, MeltQuoteState, State};
use cdk_common::state::check_state_transition;
use cdk_common::{Amount, Error, PublicKey, QuoteId};
use cdk_signatory::signatory::SignatoryKeySet;

use crate::mint::subscription::PubSubManager;
use crate::mint::MeltQuote;

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

/// Rolls back a melt quote by removing all setup artifacts and resetting state.
///
/// This function is used by both:
/// - `melt_saga::compensation::RemoveMeltSetup` when saga fails
/// - `start_up_check::rollback_failed_melt_quote` when recovering failed payments
///
/// # What This Does
///
/// Within a single database transaction:
/// 1. Removes input proofs from database
/// 2. Removes change output blinded messages
/// 3. Resets quote state from Pending to Unpaid
/// 4. Deletes melt request tracking record
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
/// Returns database errors if transaction fails
pub async fn rollback_melt_quote(
    db: &DynMintDatabase,
    quote_id: &QuoteId,
    input_ys: &[PublicKey],
    blinded_secrets: &[PublicKey],
    operation_id: &uuid::Uuid,
) -> Result<(), Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "Rolling back melt quote {} ({} proofs, {} blinded messages, saga {})",
        quote_id,
        input_ys.len(),
        blinded_secrets.len(),
        operation_id
    );

    let mut tx = db.begin_transaction().await?;

    // Remove input proofs
    if !input_ys.is_empty() {
        tx.remove_proofs(input_ys, Some(quote_id.clone())).await?;
    }

    // Remove blinded messages (change outputs)
    if !blinded_secrets.is_empty() {
        tx.delete_blinded_messages(blinded_secrets).await?;
    }

    // Get and lock the quote, then reset state from Pending to Unpaid
    if let Some(mut quote) = tx.get_melt_quote(quote_id).await? {
        let previous_state = tx
            .update_melt_quote_state(&mut quote, MeltQuoteState::Unpaid, None)
            .await?;

        if previous_state != MeltQuoteState::Pending {
            tracing::warn!(
                "Unexpected quote state during rollback: expected Pending, got {}",
                previous_state
            );
        }
    }

    // Delete melt request tracking record
    tx.delete_melt_request(quote_id).await?;

    // Delete saga state record
    if let Err(e) = tx.delete_saga(operation_id).await {
        tracing::warn!(
            "Failed to delete saga {} during rollback: {}",
            operation_id,
            e
        );
        // Continue anyway - saga cleanup is best-effort
    }

    tx.commit().await?;

    tracing::info!(
        "Successfully rolled back melt quote {} and deleted saga {}",
        quote_id,
        operation_id
    );

    Ok(())
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
/// Tuple of:
/// - `Option<Vec<BlindSignature>>` - Signed change outputs (if any)
/// - `Box<dyn MintTransaction>` - New transaction with signatures stored
///
/// # Errors
///
/// Returns error if:
/// - Change calculation fails
/// - Blind signing fails
/// - Database operations fail
pub async fn process_melt_change(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    quote_id: &QuoteId,
    inputs_amount: Amount,
    total_spent: Amount,
    inputs_fee: Amount,
    change_outputs: Vec<BlindedMessage>,
) -> Result<
    (
        Option<Vec<BlindSignature>>,
        Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    ),
    Error,
> {
    // Check if change is needed
    let needs_change = inputs_amount > total_spent;

    if !needs_change || change_outputs.is_empty() {
        // No change needed - open transaction and return empty result
        let tx = db.begin_transaction().await?;
        return Ok((None, tx));
    }

    let change_target = inputs_amount - total_spent - inputs_fee;

    // Get keyset configuration
    let fee_and_amounts = get_keyset_fee_and_amounts(&mint.keysets, &change_outputs);

    // Split change into denominations
    let mut amounts = change_target.split(&fee_and_amounts);

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

    // Open new transaction to store signatures
    let mut tx = db.begin_transaction().await?;

    let blinded_secrets: Vec<_> = blinded_messages_to_sign
        .iter()
        .map(|bm| bm.blinded_secret)
        .collect();

    tx.add_blind_signatures(&blinded_secrets, &change_sigs, Some(quote_id.clone()))
        .await?;

    Ok((Some(change_sigs), tx))
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
/// * [`Error::Database(Duplicate)`] if another quote with the same lookup ID is already pending
///   or paid, indicating a conflicting concurrent melt operation.
pub async fn load_melt_quotes_exclusively(
    tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    quote_id: &QuoteId,
) -> Result<Acquired<MeltQuote>, Error> {
    // Lock ALL related quotes in a single atomic query to prevent deadlocks.
    // The query locks quotes ordered by ID, ensuring consistent lock acquisition order
    // across concurrent transactions.
    let locked = tx
        .lock_melt_quote_and_related(quote_id)
        .await
        .map_err(|e| match e {
            database::Error::Locked => {
                tracing::warn!("Quote {quote_id} or related quotes are locked by another process");
                database::Error::Duplicate
            }
            e => e,
        })?;

    let quote = locked.target.ok_or(Error::UnknownQuote)?;

    if locked.all_related.iter().any(|locked_quote| {
        locked_quote.id != quote.id
            && (locked_quote.state == MeltQuoteState::Pending
                || locked_quote.state == MeltQuoteState::Paid)
    }) {
        tracing::warn!(
            "Cannot transition quote {} to Pending: another quote with lookup_id {:?} is already pending or paid",
            quote.id,
            quote.request_lookup_id,
        );
        return Err(Error::Database(crate::cdk_database::Error::Duplicate));
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
/// * `payment_preimage` - Payment preimage (if any)
/// * `payment_lookup_id` - Payment lookup identifier
///
/// # Returns
///
/// `Ok(())` if finalization succeeds
///
/// # Errors
///
/// Returns error if:
/// - Amount validation fails
/// - Proofs are already spent
/// - Database operations fail
#[allow(clippy::too_many_arguments)]
pub async fn finalize_melt_core(
    tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
    pubsub: &PubSubManager,
    quote: &mut Acquired<MeltQuote>,
    input_ys: &[PublicKey],
    inputs_amount: Amount,
    inputs_fee: Amount,
    total_spent: Amount,
    payment_preimage: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
) -> Result<(), Error> {
    // Validate quote amount vs payment amount
    if quote.amount > total_spent {
        tracing::error!(
            "Payment amount {} is less than quote amount {} for quote {}",
            total_spent,
            quote.amount,
            quote.id
        );
        return Err(Error::IncorrectQuoteAmount);
    }

    // Validate inputs amount
    if inputs_amount - inputs_fee < total_spent {
        tracing::error!("Over paid melt quote {}", quote.id);
        return Err(Error::IncorrectQuoteAmount);
    }

    // Update quote state to Paid
    tx.update_melt_quote_state(quote, MeltQuoteState::Paid, payment_preimage.clone())
        .await?;

    // Update payment lookup ID if changed
    if quote.request_lookup_id.as_ref() != Some(payment_lookup_id) {
        tracing::info!(
            "Payment lookup id changed post payment from {:?} to {}",
            &quote.request_lookup_id,
            payment_lookup_id
        );

        tx.update_melt_quote_request_lookup_id(quote, payment_lookup_id)
            .await?;
    }

    for current_state in tx
        .get_proofs_states(input_ys)
        .await?
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(Error::UnexpectedProofState)?
    {
        check_state_transition(current_state, State::Spent)
            .map_err(|_| Error::UnexpectedProofState)?;
    }

    // Mark input proofs as spent
    match tx.update_proofs_states(input_ys, State::Spent).await {
        Ok(_) => {}
        Err(database::Error::AttemptUpdateSpentProof) => {
            tracing::info!("Proofs for quote {} already marked as spent", quote.id);
            return Ok(());
        }
        Err(err) => {
            return Err(err.into());
        }
    }

    // Publish proof state changes
    for pk in input_ys.iter() {
        pubsub.proof_state((*pk, State::Spent));
    }

    Ok(())
}

/// High-level melt finalization that handles the complete workflow.
///
/// This function orchestrates:
/// 1. Getting melt request info
/// 2. Getting input proof Y values
/// 3. Processing change (if needed)
/// 4. Core finalization operations
/// 5. Transaction commit
/// 6. Pubsub notification
///
/// # Arguments
///
/// * `mint` - Mint instance
/// * `db` - Database connection
/// * `pubsub` - Pubsub manager
/// * `quote` - Melt quote to finalize
/// * `total_spent` - Amount spent on payment
/// * `payment_preimage` - Payment preimage (if any)
/// * `payment_lookup_id` - Payment lookup identifier
///
/// # Returns
///
/// `Option<Vec<BlindSignature>>` - Change signatures (if any)
pub async fn finalize_melt_quote(
    mint: &super::super::Mint,
    db: &DynMintDatabase,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
    total_spent: Amount,
    payment_preimage: Option<String>,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
) -> Result<Option<Vec<BlindSignature>>, Error> {
    use cdk_common::amount::to_unit;

    tracing::info!("Finalizing melt quote {}", quote.id);

    // Convert total_spent to quote unit
    let total_spent = to_unit(total_spent, &quote.unit, &quote.unit).unwrap_or(total_spent);

    let mut tx = db.begin_transaction().await?;

    // Acquire lock on the quote for safe state update

    let mut locked_quote = load_melt_quotes_exclusively(&mut tx, &quote.id).await?;

    // Get melt request info
    let melt_request_info = match tx.get_melt_request_and_blinded_messages(&quote.id).await? {
        Some(info) => info,
        None => {
            tracing::warn!(
                "No melt request found for quote {} - may have been completed already",
                quote.id
            );
            tx.rollback().await?;
            return Ok(None);
        }
    };

    // Get input proof Y values
    let input_ys = tx.get_proof_ys_by_quote_id(&quote.id).await?;

    if input_ys.is_empty() {
        tracing::warn!(
            "No input proofs found for quote {} - may have been completed already",
            quote.id
        );
        tx.rollback().await?;
        return Ok(None);
    }

    // Core finalization (marks proofs spent, updates quote)
    finalize_melt_core(
        &mut tx,
        pubsub,
        &mut locked_quote,
        &input_ys,
        melt_request_info.inputs_amount,
        melt_request_info.inputs_fee,
        total_spent,
        payment_preimage.clone(),
        payment_lookup_id,
    )
    .await?;

    // Close transaction before external call
    tx.commit().await?;

    // Process change (if needed) - opens new transaction
    let (change_sigs, mut tx) = process_melt_change(
        mint,
        db,
        &quote.id,
        melt_request_info.inputs_amount,
        total_spent,
        melt_request_info.inputs_fee,
        melt_request_info.change_outputs.clone(),
    )
    .await?;

    // Delete melt request tracking
    tx.delete_melt_request(&quote.id).await?;

    // Commit transaction
    tx.commit().await?;

    // Publish quote status change
    pubsub.melt_quote_status(
        quote,
        payment_preimage,
        change_sigs.clone(),
        MeltQuoteState::Paid,
    );

    tracing::info!("Successfully finalized melt quote {}", quote.id);

    Ok(change_sigs)
}
