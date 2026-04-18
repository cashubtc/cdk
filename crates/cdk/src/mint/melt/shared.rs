//! Shared logic for melt operations across saga and startup check.
//!
//! This module contains common functions used by both:
//! - `melt_saga`: Normal melt operation flow
//! - `start_up_check`: Recovery of interrupted melts during startup
//!
//! The functions here ensure consistency between these two code paths.

use cdk_common::database::mint::Acquired;
use cdk_common::database::{self, DynMintDatabase};
use cdk_common::mint::{self as mint_types};
use cdk_common::nuts::{BlindSignature, BlindedMessage, MeltQuoteState, Proofs, State};
use cdk_common::{Amount, CurrencyUnit, Error, PublicKey, QuoteId};
use cdk_signatory::signatory::SignatoryKeySet;

use crate::mint::subscription::PubSubManager;
use crate::mint::MeltQuote;
use crate::Mint;

// A melt quote may only be finalized once. After that, only the exact same
// backend settlement is treated as an idempotent duplicate success.
fn melt_settlement_matches(
    quote: &MeltQuote,
    payment_lookup_id: &cdk_common::payment::PaymentIdentifier,
    payment_proof: &Option<String>,
) -> bool {
    let lookup_matches = quote
        .request_lookup_id
        .as_ref()
        .is_none_or(|stored_lookup_id| stored_lookup_id == payment_lookup_id);

    lookup_matches && quote.payment_proof == *payment_proof
}

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
    pubsub: &PubSubManager,
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

    // Remove blinded messages (change outputs)
    if !blinded_secrets.is_empty() {
        tx.delete_blinded_messages(blinded_secrets).await?;
    }

    // Duplicate failure delivery can happen after rollback already completed.
    // Keep rollback idempotent by treating Unpaid as already handled and Paid
    // as terminal success that must not be rolled back.
    let quote_option = if let Some(mut quote) = tx.get_melt_quote(quote_id).await? {
        if quote.state == MeltQuoteState::Paid {
            tracing::warn!("Ignoring rollback for already-paid melt quote {}", quote_id);
            tx.rollback().await?;
            return Ok(());
        }

        if quote.state != MeltQuoteState::Unpaid {
            let previous_state = tx
                .update_melt_quote_state(&mut quote, MeltQuoteState::Unpaid, None)
                .await?;

            if previous_state != MeltQuoteState::Pending {
                tracing::warn!(
                    "Unexpected quote state during rollback: expected Pending, got {}",
                    previous_state
                );
            }
        } else {
            tracing::info!(
                "Melt quote {} already rolled back, keeping it Unpaid",
                quote_id
            );
        }
        Some(quote)
    } else {
        None
    };

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
    let change_target: Amount = match inputs_amount
        .checked_sub(&total_spent)
        .ok()
        .and_then(|rem| rem.checked_sub(&inputs_fee).ok())
    {
        Some(amt) if amt.value() > 0 => amt.into(),
        Some(_) => {
            // Exactly 0 change needed - open transaction and return empty result
            let tx = db.begin_transaction().await?;
            return Ok((None, tx));
        }
        None => {
            tracing::warn!(
                "Fee was too high for quote {}. inputs_amount: {}, total_spent: {}, inputs_fee: {}",
                quote_id,
                inputs_amount,
                total_spent,
                inputs_fee
            );
            let tx = db.begin_transaction().await?;
            return Ok((None, tx));
        }
    };

    if change_outputs.is_empty() {
        let tx = db.begin_transaction().await?;
        return Ok((None, tx));
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
/// * [`Error::PendingQuote`] (code 20005) if another quote with the same lookup ID is pending.
/// * [`Error::RequestAlreadyPaid`] (code 20006) if another quote with the same lookup ID is paid.
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

    // This can only happen on backends where we cannot set the max fee (e.g., LNbits).
    // LNbits does not allow setting a fee limit, so payments can exceed the fee reserve.
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
    tracing::info!("Finalizing melt quote {}", quote.id);

    let mut tx = db.begin_transaction().await?;

    // Acquire lock on the quote for safe state update

    let locked_quote = load_melt_quotes_exclusively(&mut tx, &quote.id).await?;

    // Async payment streams may redeliver a prior success event. If the quote
    // is already Paid, only the same settlement is allowed to no-op here.
    let already_paid_with_matching_settlement = if locked_quote.state == MeltQuoteState::Paid {
        if melt_settlement_matches(&locked_quote, payment_lookup_id, &payment_proof) {
            tracing::info!(
                "Melt quote {} already finalized with matching settlement, resuming cleanup",
                quote.id
            );
            true
        } else {
            tracing::warn!(
                "Melt quote {} already finalized with different settlement: stored lookup_id={:?}, incoming lookup_id={}, stored payment_proof={:?}, incoming payment_proof={:?}",
                quote.id,
                locked_quote.request_lookup_id,
                payment_lookup_id,
                locked_quote.payment_proof,
                payment_proof,
            );
            tx.rollback().await?;
            return Err(Error::PaidQuote);
        }
    } else {
        false
    };

    // Get melt request info
    let melt_request_info = match tx.get_melt_request_and_blinded_messages(&quote.id).await? {
        Some(info) => info,
        None => {
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

    // Check if TX1 already completed (e.g., crash between TX1 commit and TX2 commit).
    // If the quote is already Paid, proofs are already Spent — calling finalize_melt_core
    // would fail on the Paid→Paid and Spent→Spent state transitions. Skip directly to
    // change signing and cleanup so the user receives their change.
    //
    // We still need the proofs for fee calculation (operation recording), so fetch them
    // from the DB even in the already-Paid case.
    let (proofs, quote) = if already_paid_with_matching_settlement {
        let proofs = tx.get_proofs(&input_ys).await?.to_vec();
        let locked_quote = locked_quote.inner();
        tx.commit().await?;
        (proofs, locked_quote)
    } else {
        finalize_melt_core(
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
        .await?
    };

    // Process change (if needed) - opens new transaction
    let (change_sigs, mut tx) = match process_melt_change(
        mint,
        db,
        &quote.id,
        melt_request_info.inputs_amount.clone(),
        total_spent.clone(),
        melt_request_info.inputs_fee.clone(),
        melt_request_info.change_outputs.clone(),
    )
    .await
    {
        Ok(res) => res,
        Err(Error::Database(cdk_common::database::Error::Duplicate)) => {
            tracing::info!(
                "Change signatures already exist for quote {}, fetching them.",
                quote.id
            );
            let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
            let change_sigs = if sigs.is_empty() { None } else { Some(sigs) };
            (change_sigs, db.begin_transaction().await?)
        }
        Err(e) => return Err(e),
    };

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
            if matches!(err, database::Error::Duplicate) {
                tracing::info!("Completed operation already exists for quote {}", quote.id);
                let sigs = db.get_blind_signatures_for_quote(&quote.id).await?;
                tx.delete_melt_request(&quote.id).await?;
                if let Some(op_id) = operation_id {
                    tx.delete_saga(&op_id).await?;
                }
                tx.commit().await?;
                return Ok(if sigs.is_empty() { None } else { Some(sigs) });
            }
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

    tracing::info!("Successfully finalized melt quote {}", quote.id);

    Ok(change_sigs)
}
