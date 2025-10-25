//! NUT-10: Spending Conditions
//!
//! This module implements verification logic for spending conditions (NUT-10/NUT-11).
//! See: https://cashubtc.github.io/nuts/10/

use cdk_common::Proofs;

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
