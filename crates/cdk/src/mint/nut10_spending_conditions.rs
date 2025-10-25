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
    _inputs: &Proofs,
    _outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // TODO: Implement SIG_ALL verification
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
