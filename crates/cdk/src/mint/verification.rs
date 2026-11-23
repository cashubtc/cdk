use std::collections::{HashMap, HashSet};

use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::nut17::MAX_CUSTOM_KIND_LEN;
use cdk_common::{
    Amount, BlindedMessage, CurrencyUnit, Id, PaymentMethod, Proofs, ProofsMethods, PublicKey,
};
use tracing::instrument;

use super::{Error, Mint};
use crate::fees::{calculate_fee, ProofsFeeBreakdown};

/// Maximum allowed length in bytes for proof secret or witness content
pub(crate) const MAX_PROOF_CONTENT_LEN: usize = 1024;

/// Maximum allowed length in bytes for request fields (description, request, extra)
pub(crate) const MAX_REQUEST_FIELD_LEN: usize = 1024;

// Both custom quote kind suffixes (`_mint_quote` and `_melt_quote`) are 11 bytes.
const CUSTOM_QUOTE_KIND_SUFFIX_LEN: usize = "_mint_quote".len();
pub(crate) const MAX_CUSTOM_PAYMENT_METHOD_LEN: usize =
    MAX_CUSTOM_KIND_LEN - CUSTOM_QUOTE_KIND_SUFFIX_LEN;

pub(crate) fn validate_custom_payment_method(method: &PaymentMethod) -> Result<(), Error> {
    match method {
        PaymentMethod::Custom(method) if method.len() > MAX_CUSTOM_PAYMENT_METHOD_LEN => {
            Err(Error::RequestFieldTooLarge {
                field: "method".to_string(),
                actual: method.len(),
                max: MAX_CUSTOM_PAYMENT_METHOD_LEN,
            })
        }
        _ => Ok(()),
    }
}

/// Verification result with typed amount
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Verification {
    /// Verified amount with unit
    pub amount: Amount<CurrencyUnit>,
}

pub(super) fn validate_proof_content_bounds(proofs: &Proofs) -> Result<(), Error> {
    for proof in proofs {
        let secret_len = proof.secret.len();
        if secret_len > MAX_PROOF_CONTENT_LEN {
            tracing::warn!(
                "Proof secret exceeds max content length: {} > {}",
                secret_len,
                MAX_PROOF_CONTENT_LEN
            );
            return Err(Error::ProofContentTooLarge {
                actual: secret_len,
                max: MAX_PROOF_CONTENT_LEN,
            });
        }

        if let Some(witness) = &proof.witness {
            let witness_str = serde_json::to_string(witness)?;
            let witness_len = witness_str.len();
            if witness_len > MAX_PROOF_CONTENT_LEN {
                tracing::warn!(
                    "Proof witness exceeds max content length: {} > {}",
                    witness_len,
                    MAX_PROOF_CONTENT_LEN
                );
                return Err(Error::ProofContentTooLarge {
                    actual: witness_len,
                    max: MAX_PROOF_CONTENT_LEN,
                });
            }
        }
    }

    Ok(())
}

/// Keysets a transaction is verified against.
///
/// The standalone mint reads them from its signatory snapshot. A federation
/// member reads them from the consensus-derived config, which also restricts
/// the outputs it may sign. Every rule in this module runs against this view,
/// so both modes share one implementation.
pub(crate) trait KeysetView {
    /// Keyset metadata for `id`, if this source knows the keyset.
    fn keyset_info(&self, id: &Id) -> Option<MintKeySetInfo>;

    /// Checks one output against rules specific to this keyset source.
    ///
    /// Runs before the shared keyset checks for that output.
    fn check_output(&self, _output: &BlindedMessage) -> Result<(), Error> {
        Ok(())
    }
}

impl KeysetView for Mint {
    fn keyset_info(&self, id: &Id) -> Option<MintKeySetInfo> {
        self.get_keyset_info(id)
    }
}

/// Returns the single unit of `outputs`.
///
/// Every output must target a known, active and unexpired keyset.
pub(crate) fn verify_outputs_keyset<K>(
    keysets: &K,
    outputs: &[BlindedMessage],
) -> Result<CurrencyUnit, Error>
where
    K: KeysetView + ?Sized,
{
    let mut checked_keysets = HashSet::new();
    let mut keyset_units = HashSet::new();

    for output in outputs {
        keysets.check_output(output)?;

        let id = output.keyset_id;
        if !checked_keysets.insert(id) {
            continue;
        }

        match keysets.keyset_info(&id) {
            Some(keyset) => {
                if !keyset.active {
                    tracing::debug!(
                        "Transaction attempted with inactive keyset in outputs: {}.",
                        id
                    );
                    return Err(Error::InactiveKeyset);
                }
                if keyset.is_expired() {
                    tracing::debug!(
                        "Transaction attempted with expired keyset in outputs: {}.",
                        id
                    );
                    return Err(Error::ExpiredKeyset);
                }
                keyset_units.insert(keyset.unit);
            }
            None => {
                tracing::debug!(
                    "Transaction attempted with unknown keyset in outputs: {}.",
                    id
                );
                return Err(Error::UnknownKeySet);
            }
        }
    }

    // Check that all outputs are the same unit
    // in the future it maybe possible to support multiple units but unsupported for
    // now
    if keyset_units.len() != 1 {
        tracing::debug!(
            "Transaction attempted with multiple units in outputs: {:?}.",
            keyset_units
        );
        return Err(Error::MultipleUnits);
    }

    keyset_units.into_iter().next().ok_or(Error::Internal)
}

/// Returns the single unit of `inputs`.
///
/// Every input must come from a known, unexpired keyset.
pub(crate) fn verify_inputs_keyset<K>(keysets: &K, inputs: &Proofs) -> Result<CurrencyUnit, Error>
where
    K: KeysetView + ?Sized,
{
    let mut checked_keysets = HashSet::new();
    let mut keyset_units = HashSet::new();

    for proof in inputs {
        let id = proof.keyset_id;
        if !checked_keysets.insert(id) {
            continue;
        }

        match keysets.keyset_info(&id) {
            Some(keyset) => {
                if keyset.is_expired() {
                    tracing::debug!(
                        "Transaction attempted with expired keyset in inputs: {}.",
                        id
                    );
                    return Err(Error::ExpiredKeyset);
                }
                keyset_units.insert(keyset.unit);
            }
            None => {
                tracing::debug!(
                    "Transaction attempted with unknown keyset in inputs: {}.",
                    id
                );
                return Err(Error::UnknownKeySet);
            }
        }
    }

    // Check that all proofs are the same unit
    // in the future it maybe possible to support multiple units but unsupported for
    // now
    if keyset_units.len() != 1 {
        tracing::debug!(
            "Transaction attempted with multiple units in inputs: {:?}.",
            keyset_units
        );
        return Err(Error::MultipleUnits);
    }

    keyset_units.into_iter().next().ok_or(Error::Internal)
}

/// Verifies outputs.
///
/// Checks outputs are non-empty, within `max_outputs`, unique and of one unit.
/// Does not check whether they were signed before.
pub(crate) fn verify_outputs<K>(
    keysets: &K,
    max_outputs: usize,
    outputs: &[BlindedMessage],
) -> Result<Verification, Error>
where
    K: KeysetView + ?Sized,
{
    if outputs.is_empty() {
        tracing::debug!("verify_outputs called with empty outputs");
        return Err(Error::TransactionUnbalanced(0, 0, 0));
    }

    let outputs_count = outputs.len();
    if outputs_count > max_outputs {
        tracing::warn!(
            "Request exceeds max outputs limit: {} > {}",
            outputs_count,
            max_outputs
        );
        return Err(Error::MaxOutputsExceeded {
            actual: outputs_count,
            max: max_outputs,
        });
    }

    Mint::check_outputs_unique(outputs)?;

    let unit = verify_outputs_keyset(keysets, outputs)?;

    let amount = Amount::try_sum(outputs.iter().map(|o| o.amount))?.with_unit(unit);

    Ok(Verification { amount })
}

/// Checks inputs are within `max_inputs`, within content bounds and unique.
///
/// These checks need no keyset and run before signature verification.
pub(crate) fn verify_inputs_bounds(max_inputs: usize, inputs: &Proofs) -> Result<(), Error> {
    let inputs_count = inputs.len();
    if inputs_count > max_inputs {
        tracing::warn!(
            "Request exceeds max inputs limit: {} > {}",
            inputs_count,
            max_inputs
        );
        return Err(Error::MaxInputsExceeded {
            actual: inputs_count,
            max: max_inputs,
        });
    }

    validate_proof_content_bounds(inputs)?;

    Mint::check_inputs_unique(inputs)
}

/// Returns the total amount of `inputs` in their single, spendable unit.
///
/// **NOTE: This checks neither signatures nor whether inputs have been spent.
pub(crate) fn verify_inputs_amount<K>(keysets: &K, inputs: &Proofs) -> Result<Verification, Error>
where
    K: KeysetView + ?Sized,
{
    let unit = verify_inputs_keyset(keysets, inputs)?;

    if unit == CurrencyUnit::Auth {
        return Err(Error::UnsupportedUnit);
    }

    let amount = inputs.total_amount()?.with_unit(unit);

    Ok(Verification { amount })
}

/// Fee required for a proof set.
pub(crate) fn calculate_proofs_fee<K>(
    keysets: &K,
    proofs: &Proofs,
) -> Result<ProofsFeeBreakdown, Error>
where
    K: KeysetView + ?Sized,
{
    let mut proofs_per_keyset = HashMap::new();
    let mut fee_per_keyset = HashMap::new();

    for proof in proofs {
        if let std::collections::hash_map::Entry::Vacant(e) = fee_per_keyset.entry(proof.keyset_id)
        {
            let keyset = keysets
                .keyset_info(&proof.keyset_id)
                .ok_or(Error::UnknownKeySet)?;
            e.insert(keyset.input_fee_ppk);
        }

        proofs_per_keyset
            .entry(proof.keyset_id)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }

    calculate_fee(&proofs_per_keyset, &fee_per_keyset)
}

/// Verifies that inputs equal outputs plus `fee_breakdown`, in one unit.
pub(crate) fn verify_balanced(
    input_verification: &Verification,
    output_verification: &Verification,
    fee_breakdown: &ProofsFeeBreakdown,
) -> Result<(), Error> {
    // Units are embedded in the typed amounts - check they match
    if output_verification.amount.unit() != input_verification.amount.unit() {
        tracing::debug!(
            "Output unit {:?} does not match input unit {:?}",
            output_verification.amount.unit(),
            input_verification.amount.unit()
        );
        return Err(Error::UnitMismatch);
    }

    // Check amounts are balanced (inputs = outputs + fee)
    let fee_typed = fee_breakdown
        .total
        .with_unit(input_verification.amount.unit().clone());
    let expected_output = input_verification.amount.checked_sub(&fee_typed)?;

    if output_verification.amount != expected_output {
        return Err(Error::TransactionUnbalanced(
            input_verification.amount.value(),
            output_verification.amount.value(),
            fee_breakdown.total.into(),
        ));
    }

    Ok(())
}

impl Mint {
    /// Verify that the inputs to the transaction are unique
    #[instrument(skip_all)]
    pub fn check_inputs_unique(inputs: &Proofs) -> Result<(), Error> {
        let proof_count = inputs.len();

        if inputs
            .iter()
            .map(|i| i.y())
            .collect::<Result<HashSet<PublicKey>, _>>()?
            .len()
            .ne(&proof_count)
        {
            tracing::debug!("Transaction attempted with duplicate inputs");
            return Err(Error::DuplicateInputs);
        }

        Ok(())
    }

    /// Verify that the outputs to are unique
    #[instrument(skip_all)]
    pub fn check_outputs_unique(outputs: &[BlindedMessage]) -> Result<(), Error> {
        let output_count = outputs.len();

        if outputs
            .iter()
            .map(|o| &o.blinded_secret)
            .collect::<HashSet<&PublicKey>>()
            .len()
            .ne(&output_count)
        {
            tracing::debug!("Transaction attempted with duplicate outputs");
            return Err(Error::DuplicateOutputs);
        }

        Ok(())
    }

    /// Verify output keyset
    ///
    /// Checks that the outputs are all of the same unit and the keyset is active
    #[instrument(skip_all)]
    pub fn verify_outputs_keyset(&self, outputs: &[BlindedMessage]) -> Result<CurrencyUnit, Error> {
        verify_outputs_keyset(self, outputs)
    }

    /// Verify input keyset
    ///
    /// Checks that the inputs are all of the same unit
    #[instrument(skip_all)]
    pub async fn verify_inputs_keyset(&self, inputs: &Proofs) -> Result<CurrencyUnit, Error> {
        verify_inputs_keyset(self, inputs)
    }

    /// Verifies outputs
    ///
    /// Checks outputs are unique, of the same unit and not signed before.
    /// Returns an error if outputs are empty - callers should guard against
    /// empty outputs before calling this function.
    #[instrument(skip_all)]
    pub fn verify_outputs(&self, outputs: &[BlindedMessage]) -> Result<Verification, Error> {
        verify_outputs(self, self.max_outputs, outputs)
    }

    /// Verifies inputs
    ///
    /// Checks that inputs are unique and of the same unit.
    /// **NOTE: This does not check if inputs have been spent
    #[instrument(skip_all)]
    pub async fn verify_inputs(&self, inputs: &Proofs) -> Result<Verification, Error> {
        verify_inputs_bounds(self.max_inputs, inputs)?;

        let verification = verify_inputs_amount(self, inputs)?;

        self.verify_proofs(inputs.clone()).await?;

        Ok(verification)
    }

    /// Verify that inputs and outputs are valid and balanced
    #[instrument(skip_all)]
    pub async fn verify_transaction_balanced(
        &self,
        input_verification: Verification,
        output_verification: Verification,
        inputs: &Proofs,
    ) -> Result<(), Error> {
        let fee_breakdown = self.get_proofs_fee(inputs).await?;

        verify_balanced(&input_verification, &output_verification, &fee_breakdown)
    }
}
