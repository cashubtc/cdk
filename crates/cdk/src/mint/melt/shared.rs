//! Shared logic for melt operations across saga and startup check.
//!
//! This module contains common functions used by both:
//! - `melt_saga`: Normal melt operation flow
//! - `start_up_check`: Recovery of interrupted melts during startup
//!
//! The functions here ensure consistency between these two code paths.

use cdk_common::database::{self, DynMintDatabase};
use cdk_common::nuts::{BlindSignature, BlindedMessage, MeltQuoteState, State};
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
) -> Result<(), Error> {
    if input_ys.is_empty() && blinded_secrets.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "Rolling back melt quote {} ({} proofs, {} blinded messages)",
        quote_id,
        input_ys.len(),
        blinded_secrets.len()
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

    // Reset quote state from Pending to Unpaid
    let (previous_state, _quote) = tx
        .update_melt_quote_state(quote_id, MeltQuoteState::Unpaid, None)
        .await?;

    if previous_state != MeltQuoteState::Pending {
        tracing::warn!(
            "Unexpected quote state during rollback: expected Pending, got {}",
            previous_state
        );
    }

    // Delete melt request tracking record
    tx.delete_melt_request(quote_id).await?;

    tx.commit().await?;

    tracing::info!("Successfully rolled back melt quote {}", quote_id);

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
pub async fn process_melt_change<'a>(
    mint: &super::super::Mint,
    db: &'a DynMintDatabase,
    quote_id: &QuoteId,
    inputs_amount: Amount,
    total_spent: Amount,
    inputs_fee: Amount,
    change_outputs: Vec<BlindedMessage>,
) -> Result<
    (
        Option<Vec<BlindSignature>>,
        Box<dyn database::MintTransaction<'a, database::Error> + Send + Sync + 'a>,
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
    tx: &mut Box<dyn database::MintTransaction<'_, database::Error> + Send + Sync + '_>,
    pubsub: &PubSubManager,
    quote: &MeltQuote,
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
    tx.update_melt_quote_state(&quote.id, MeltQuoteState::Paid, payment_preimage.clone())
        .await?;

    // Update payment lookup ID if changed
    if quote.request_lookup_id.as_ref() != Some(payment_lookup_id) {
        tracing::info!(
            "Payment lookup id changed post payment from {:?} to {}",
            &quote.request_lookup_id,
            payment_lookup_id
        );

        tx.update_melt_quote_request_lookup_id(&quote.id, payment_lookup_id)
            .await?;
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
        quote,
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
