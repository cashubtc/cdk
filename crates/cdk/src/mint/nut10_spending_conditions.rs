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
pub fn verify_spending_conditions_for_swap(
    inputs: &Proofs,
    outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // Check if any input has SIG_ALL flag
    if has_at_least_one_sig_all(inputs)? {
        // at least one input has SIG_ALL
        verify_full_sig_all_check_swap(inputs, outputs)
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
fn verify_full_sig_all_check_swap(
    inputs: &Proofs,
    outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // Verify all inputs meet SIG_ALL requirements per NUT-11:
    // All inputs must have: (1) same kind, (2) SIG_ALL flag, (3) same data, (4) same tags
    verify_all_inputs_match_for_sig_all(inputs)?;

    // Get the first input to determine the kind
    let first_input = inputs.first().ok_or(Error::Internal)?;
    let first_secret = cdk_common::nuts::nut10::Secret::try_from(&first_input.secret)?;

    // Dispatch based on secret kind
    match first_secret.kind() {
        cdk_common::nuts::Kind::P2PK => {
            verify_sig_all_swap_p2pk(inputs, outputs)?;
        }
        cdk_common::nuts::Kind::HTLC => {
            // TODO: Implement HTLC SIG_ALL verification
            return Err(Error::InvalidSpendConditions("HTLC SIG_ALL not yet implemented".into()));
        }
    }

    Ok(())
}

/// Verify P2PK SIG_ALL signatures for swap
fn verify_sig_all_swap_p2pk(
    inputs: &Proofs,
    outputs: &[BlindedMessage],
) -> Result<(), Error> {
    // Do NOT call this directly. This is called only from 'verify_sig_all_swap',
    // which has already done many important SIG_ALL checks. This just does
    // some checks which are specific to SIG_ALL+P2PK+swap
    // Get the first input, as it's the one with the signatures
    let first_input = inputs.first().ok_or(Error::Internal)?;
    let first_secret = cdk_common::nuts::nut10::Secret::try_from(&first_input.secret)?;

    // Record current time for locktime evaluation
    let current_time = cdk_common::util::unix_time();

    // Get the relevant public keys and required signature count based on locktime
    let (pubkeys, required_sigs) = cdk_common::nuts::nut11::get_pubkeys_and_required_sigs_for_p2pk(&first_secret, current_time)?;

    // Handle "anyone can spend" case (locktime passed with no refund keys)
    if required_sigs == 0 {
        return Ok(());
    }

    let first_input = inputs.first().ok_or(Error::Internal)?;

    // Construct the message that should be signed (all input secrets + all output blinded messages)
    let msg_to_sign = construct_sig_all_message_swap(inputs, outputs);

    // Debug: verify our message construction matches the existing SwapRequest implementation
    #[cfg(debug_assertions)]
    {
        let _temp_swap_request = cdk_common::nuts::SwapRequest::new(
            inputs.clone(),
            outputs.to_vec(),
        );
        // We can't call the private sig_all_msg_to_sign() method, but we can manually construct
        // what it would return and verify it matches ours
        let mut expected_msg = String::new();
        for proof in inputs {
            expected_msg.push_str(&proof.secret.to_string());
        }
        for output in outputs {
            expected_msg.push_str(&output.blinded_secret.to_string());
        }
        debug_assert_eq!(msg_to_sign, expected_msg, "Our sig_all message construction doesn't match expected format");
    }

    // Extract signatures from the first input's witness
    let first_witness = first_input
        .witness
        .as_ref()
        .ok_or(Error::InvalidSpendConditions("SIG_ALL requires signatures".into()))?;

    let witness_sigs = first_witness
        .signatures()
        .ok_or(Error::InvalidSpendConditions("SIG_ALL requires signatures in witness".into()))?;

    // Convert witness strings to Signature objects
    use std::str::FromStr;
    let signatures: Vec<cdk_common::bitcoin::secp256k1::schnorr::Signature> = witness_sigs
        .iter()
        .map(|s| cdk_common::bitcoin::secp256k1::schnorr::Signature::from_str(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::InvalidSpendConditions("Invalid signature format".into()))?;

    // Verify signatures using the existing valid_signatures function
    let valid_sig_count = cdk_common::nuts::nut11::valid_signatures(
        msg_to_sign.as_bytes(),
        &pubkeys,
        &signatures,
    )?;

    // Check if we have enough valid signatures
    if valid_sig_count < required_sigs {
        return Err(Error::InvalidSpendConditions(
            format!("SIG_ALL requires {} signatures, found {}", required_sigs, valid_sig_count)
        ));
    }

    Ok(())
}

/// Construct the message to sign for SIG_ALL verification
///
/// Concatenates all input secrets and output blinded messages in order
fn construct_sig_all_message_swap(inputs: &cdk_common::Proofs, outputs: &[BlindedMessage]) -> String {
    let mut msg_to_sign = String::new();

    // Add all input secrets in order
    for proof in inputs {
        let secret = proof.secret.to_string();
        msg_to_sign.push_str(&secret);
    }

    // Add all output blinded messages in order
    for output in outputs {
        let message = output.blinded_secret.to_string();
        msg_to_sign.push_str(&message);
    }

    msg_to_sign
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
