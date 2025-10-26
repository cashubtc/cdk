//! NUT-03: Swap
//!
//! <https://github.com/cashubtc/nuts/blob/main/03.md>

use bitcoin::hashes::Hash;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "wallet")]
use super::nut00::PreMintSecrets;
use super::nut00::{BlindSignature, BlindedMessage, Proofs};
use super::ProofsMethods;
use crate::Amount;

/// NUT03 Error
#[derive(Debug, Error)]
pub enum Error {
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] crate::amount::Error),
}

/// Preswap information
#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreSwap {
    /// Preswap mint secrets
    pub pre_mint_secrets: PreMintSecrets,
    /// Swap request
    pub swap_request: SwapRequest,
    /// Amount to increment keyset counter by
    pub derived_secret_count: u32,
    /// Fee amount
    pub fee: Amount,
}

/// Swap Request [NUT-03]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SwapRequest {
    /// Proofs that are to be spent in a `Swap`
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<crate::Proof>))]
    inputs: Proofs,
    /// Blinded Messages for Mint to sign
    outputs: Vec<BlindedMessage>,
}

impl SwapRequest {
    /// Create new [`SwapRequest`]
    pub fn new(inputs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self {
            inputs: inputs.without_dleqs(),
            outputs,
        }
    }

    /// Get inputs (proofs)
    pub fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    /// Get mutable inputs (proofs)
    pub fn inputs_mut(&mut self) -> &mut Proofs {
        &mut self.inputs
    }

    /// Get outputs (blinded messages)
    pub fn outputs(&self) -> &Vec<BlindedMessage> {
        &self.outputs
    }

    /// Get mutable reference to outputs (blinded messages)
    pub fn outputs_mut(&mut self) -> &mut Vec<BlindedMessage> {
        &mut self.outputs
    }

    /// Total value of proofs in [`SwapRequest`]
    pub fn input_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.inputs.iter().map(|proof| proof.amount),
        )?)
    }

    /// Total value of outputs in [`SwapRequest`]
    pub fn output_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.outputs.iter().map(|proof| proof.amount),
        )?)
    }
}

impl super::nut10::VerificationForSpendingConditions for SwapRequest {
    fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    fn sig_all_msg_to_sign(&self) -> String {
        let mut msg = String::new();

        // Add all input secrets in order
        for proof in &self.inputs {
            msg.push_str(&proof.secret.to_string());
        }

        // Add all output blinded messages in order
        for output in &self.outputs {
            msg.push_str(&output.blinded_secret.to_string());
        }

        msg
    }
}

impl SwapRequest {
    /// Verify spending conditions for this swap transaction
    ///
    /// This is the main entry point for spending condition verification.
    /// It checks if any input has SIG_ALL and dispatches to the appropriate verification path.
    pub fn verify_spending_conditions(&self) -> Result<(), super::nut11::Error> {
        use super::nut10::VerificationForSpendingConditions;

        // Check if any input has SIG_ALL flag
        if self.has_at_least_one_sig_all()? {
            // at least one input has SIG_ALL
            self.verify_full_sig_all_check()
        } else {
            // none of the inputs are SIG_ALL, so we can simply check
            // each independently and verify any spending conditions
            // that may - or may not - be there.
            self.verify_inputs_individually().map_err(|e| match e {
                super::nut14::Error::NUT11(nut11_err) => nut11_err,
                _ => super::nut11::Error::SpendConditionsNotMet,
            })
        }
    }

    /// Verify spending conditions when SIG_ALL is present
    ///
    /// When SIG_ALL is set, all proofs in the transaction must be signed together.
    fn verify_full_sig_all_check(&self) -> Result<(), super::nut11::Error> {
        use super::nut10::VerificationForSpendingConditions;

        // Verify all inputs meet SIG_ALL requirements per NUT-11:
        // All inputs must have: (1) same kind, (2) SIG_ALL flag, (3) same data, (4) same tags
        self.verify_all_inputs_match_for_sig_all()?;

        // Get the first input to determine the kind
        let first_input = self.inputs.first().ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = super::nut10::Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Dispatch based on secret kind
        match first_secret.kind() {
            super::Kind::P2PK => {
                self.verify_sig_all_p2pk()?;
            }
            super::Kind::HTLC => {
                self.verify_sig_all_htlc()?;
            }
        }

        Ok(())
    }

    /// Verify P2PK SIG_ALL signatures for swap
    fn verify_sig_all_p2pk(&self) -> Result<(), super::nut11::Error> {
        use super::nut10::VerificationForSpendingConditions;

        // Do NOT call this directly. This is called only from 'verify_full_sig_all_check',
        // which has already done many important SIG_ALL checks. This just does
        // some checks which are specific to SIG_ALL+P2PK+swap

        // Get the first input, as it's the one with the signatures
        let first_input = self.inputs.first().ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = super::nut10::Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Record current time for locktime evaluation
        let current_time = crate::util::unix_time();

        // Get the relevant public keys and required signature count based on locktime
        let (preimage_needed, pubkeys, required_sigs) = super::nut10::get_pubkeys_and_required_sigs(&first_secret, current_time)?;

        debug_assert!(!preimage_needed, "P2PK should never require preimage");

        // Handle "anyone can spend" case (locktime passed with no refund keys)
        if required_sigs == 0 {
            return Ok(());
        }

        // Construct the message that should be signed (all input secrets + all output blinded messages)
        let msg_to_sign = self.sig_all_msg_to_sign();

        // Extract signatures from the first input's witness
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        // Convert witness strings to Signature objects
        use std::str::FromStr;
        let signatures: Vec<bitcoin::secp256k1::schnorr::Signature> = witness_sigs
            .iter()
            .map(|s| bitcoin::secp256k1::schnorr::Signature::from_str(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| super::nut11::Error::InvalidSignature)?;

        // Verify signatures using the existing valid_signatures function
        let valid_sig_count = super::nut11::valid_signatures(
            msg_to_sign.as_bytes(),
            &pubkeys,
            &signatures,
        )?;

        // Check if we have enough valid signatures
        if valid_sig_count < required_sigs {
            return Err(super::nut11::Error::SpendConditionsNotMet);
        }

        Ok(())
    }

    /// Verify HTLC SIG_ALL signatures for swap
    fn verify_sig_all_htlc(&self) -> Result<(), super::nut11::Error> {
        use super::nut10::VerificationForSpendingConditions;

        // Do NOT call this directly. This is called only from 'verify_full_sig_all_check',
        // which has already done many important SIG_ALL checks. This just does
        // some checks which are specific to SIG_ALL+HTLC+swap

        // Get the first input, as it's the one with the signatures
        let first_input = self.inputs.first().ok_or(super::nut11::Error::SpendConditionsNotMet)?;
        let first_secret = super::nut10::Secret::try_from(&first_input.secret)
            .map_err(|_| super::nut11::Error::IncorrectSecretKind)?;

        // Record current time for locktime evaluation
        let current_time = crate::util::unix_time();

        // Get the relevant public keys, required signature count, and whether preimage is needed
        let (preimage_needed, pubkeys, required_sigs) = super::nut10::get_pubkeys_and_required_sigs(&first_secret, current_time)?;

        // If preimage is needed (before locktime), verify it
        if preimage_needed {
            let hash_lock = bitcoin::hashes::sha256::Hash::from_str(first_secret.secret_data().data())
                .map_err(|_| super::nut11::Error::InvalidHash)?;

            // Extract HTLC witness
            let first_witness = first_input
                .witness
                .as_ref()
                .ok_or(super::nut11::Error::SignaturesNotProvided)?;

            let preimage = first_witness
                .preimage()
                .ok_or(super::nut11::Error::SpendConditionsNotMet)?;

            let hash_of_preimage = bitcoin::hashes::sha256::Hash::hash(preimage.as_bytes());

            if hash_lock != hash_of_preimage {
                return Err(super::nut11::Error::SpendConditionsNotMet);
            }
        }

        // Handle "anyone can spend" case (locktime passed with no refund keys)
        if required_sigs == 0 {
            return Ok(());
        }

        // Construct the message that should be signed (all input secrets + all output blinded messages)
        let msg_to_sign = self.sig_all_msg_to_sign();

        // Extract signatures from the first input's witness
        let first_witness = first_input
            .witness
            .as_ref()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        let witness_sigs = first_witness
            .signatures()
            .ok_or(super::nut11::Error::SignaturesNotProvided)?;

        // Convert witness strings to Signature objects
        use std::str::FromStr;
        let signatures: Vec<bitcoin::secp256k1::schnorr::Signature> = witness_sigs
            .iter()
            .map(|s| bitcoin::secp256k1::schnorr::Signature::from_str(s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| super::nut11::Error::InvalidSignature)?;

        // Verify signatures using the existing valid_signatures function
        let valid_sig_count = super::nut11::valid_signatures(
            msg_to_sign.as_bytes(),
            &pubkeys,
            &signatures,
        )?;

        // Check if we have enough valid signatures
        if valid_sig_count < required_sigs {
            return Err(super::nut11::Error::SpendConditionsNotMet);
        }

        Ok(())
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct SwapResponse {
    /// Promises
    pub signatures: Vec<BlindSignature>,
}

impl SwapResponse {
    /// Create new [`SwapResponse`]
    pub fn new(promises: Vec<BlindSignature>) -> Self {
        Self {
            signatures: promises,
        }
    }

    /// Total [`Amount`] of promises
    pub fn promises_amount(&self) -> Result<Amount, Error> {
        Ok(Amount::try_sum(
            self.signatures
                .iter()
                .map(|BlindSignature { amount, .. }| *amount),
        )?)
    }
}
