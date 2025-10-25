//! NUT-10: Spending Conditions
//!
//! This module implements verification logic for spending conditions (NUT-10/NUT-11).
//! See: https://cashubtc.github.io/nuts/10/

use cdk_common::{BlindedMessage, Proofs};

use super::Error;

/// Check if at least one proof in the set has SIG_ALL flag set
///
/// SIG_ALL requires all proofs in the transaction to be signed.
/// If any proof has this flag, we need to verify signatures on all proofs.
pub fn has_at_least_one_sig_all(proofs: &Proofs) -> Result<bool, Error> {
    for proof in proofs {
        // Try to extract spending conditions from the proof's secret
        if let Ok(spending_conditions) = cdk_common::nuts::SpendingConditions::try_from(&proof.secret) {
            // Check for SIG_ALL flag in either P2PK or HTLC conditions
            let has_sig_all = match spending_conditions {
                cdk_common::nuts::SpendingConditions::P2PKConditions { conditions, .. } => {
                    conditions.map(|c| c.sig_flag == cdk_common::nuts::SigFlag::SigAll).unwrap_or(false)
                }
                cdk_common::nuts::SpendingConditions::HTLCConditions { conditions, .. } => {
                    conditions.map(|c| c.sig_flag == cdk_common::nuts::SigFlag::SigAll).unwrap_or(false)
                }
            };

            if has_sig_all {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Verify spending conditions for a swap transaction
///
/// This is the main entry point for spending condition verification.
/// It checks if any input has SIG_ALL and dispatches to the appropriate verification path.
pub fn verify_spending_conditions(
    inputs: &Proofs,
    outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // Check if any input has SIG_ALL flag
    if has_at_least_one_sig_all(inputs)? {
        // at least one input has SIG_ALL
        verify_full_sig_all_check(inputs, outputs)
    } else {
        // none of the inputs are SIG_ALL, so we can simply check
        // each independently and verify any spending conditions
        // that may - or may not - be there.
        verify_inputs_individually(inputs)
    }
}

/// Verify spending conditions when SIG_ALL is present
///
/// When SIG_ALL is set, all proofs in the transaction must be signed together.
fn verify_full_sig_all_check(
    inputs: &Proofs,
    _outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // Verify all inputs meet SIG_ALL requirements per NUT-11:
    // All inputs must have: (1) same kind, (2) SIG_ALL flag, (3) same data, (4) same tags
    verify_all_inputs_match_for_sig_all(inputs)?;

    // TODO: Implement SIG_ALL signature verification
    Ok(())
}

/// Verify all inputs meet SIG_ALL requirements per NUT-11
///
/// When any input has SIG_ALL, all inputs must have:
/// 1. Same kind (P2PK or HTLC)
/// 2. SIG_ALL flag set
/// 3. Same Secret.data
/// 4. Same Secret.tags
fn verify_all_inputs_match_for_sig_all(inputs: &Proofs) -> Result<(), Error> {
    if inputs.is_empty() {
        return Err(Error::Internal);
    }

    // Get first input's properties
    let first_input = inputs.first().ok_or(Error::Internal)?;
    let first_secret = cdk_common::nuts::nut10::Secret::try_from(&first_input.secret)?;
    let first_kind = first_secret.kind();
    let first_data = first_secret.secret_data().data();
    let first_tags = first_secret.secret_data().tags();

    // Get first input's conditions to check SIG_ALL flag
    let first_conditions = cdk_common::nuts::Conditions::try_from(
        first_tags.cloned().unwrap_or_default()
    )?;

    // Verify first input has SIG_ALL (it should, since we only call this function when SIG_ALL is detected)
    if first_conditions.sig_flag != cdk_common::nuts::SigFlag::SigAll {
        return Err(Error::Internal);
    }

    // Verify all remaining inputs match
    for proof in inputs.iter().skip(1) {
        let secret = cdk_common::nuts::nut10::Secret::try_from(&proof.secret)?;

        // Check kind matches
        if secret.kind() != first_kind {
            return Err(Error::InvalidSpendConditions("All inputs must have same kind for SIG_ALL".into()));
        }

        // Check data matches
        if secret.secret_data().data() != first_data {
            return Err(Error::InvalidSpendConditions("All inputs must have same data for SIG_ALL".into()));
        }

        // Check tags match (this also ensures SIG_ALL flag matches, since sig_flag is part of tags)
        if secret.secret_data().tags() != first_tags {
            return Err(Error::InvalidSpendConditions("All inputs must have same tags for SIG_ALL".into()));
        }
    }

    Ok(())
}

/// Verify spending conditions for each input individually
///
/// Handles SIG_INPUTS mode, non-NUT-10 secrets, and any other case where inputs
/// are verified independently rather than as a group.
/// This function will NOT be called if any input has SIG_ALL; see 'verify_spending_conditions'
fn verify_inputs_individually(inputs: &Proofs) -> Result<(), Error> {
    for proof in inputs {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) = cdk_common::nuts::nut10::Secret::try_from(&proof.secret) {
            // Verify this function isn't being called with SIG_ALL proofs (development check)
            if let Ok(conditions) = cdk_common::nuts::Conditions::try_from(
                secret.secret_data().tags().cloned().unwrap_or_default()
            ) {
                debug_assert!(
                    conditions.sig_flag != cdk_common::nuts::SigFlag::SigAll,
                    "verify_inputs_individually called with SIG_ALL proof - this is a bug"
                );
            }

            match secret.kind() {
                cdk_common::nuts::Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                cdk_common::nuts::Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
        }
        // If not a nut10 secret, skip verification (plain secret)
    }
    Ok(())
}
