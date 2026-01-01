use std::collections::HashSet;

use cdk_common::{Amount, BlindedMessage, CurrencyUnit, Id, Proofs, ProofsMethods, PublicKey};
use tracing::instrument;

use super::{Error, Mint};
use crate::cdk_database;

/// Verification result with typed amount
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Verification {
    /// Verified amount with unit
    pub amount: Amount<CurrencyUnit>,
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
    ///
    /// # Panics
    ///
    /// This function will not panic as the iterator is guaranteed to have exactly one element
    /// when it reaches the expect call (length is checked above).
    #[instrument(skip_all)]
    pub fn verify_outputs_keyset(&self, outputs: &[BlindedMessage]) -> Result<CurrencyUnit, Error> {
        let mut keyset_units = HashSet::new();

        let output_keyset_ids: HashSet<Id> = outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            match self.get_keyset_info(id) {
                Some(keyset) => {
                    if !keyset.active {
                        tracing::debug!(
                            "Transaction attempted with inactive keyset in outputs: {}.",
                            id
                        );
                        return Err(Error::InactiveKeyset);
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

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len() != 1 {
            tracing::debug!(
                "Transaction attempted with multiple units in outputs: {:?}.",
                keyset_units
            );
            return Err(Error::MultipleUnits);
        }

        Ok(keyset_units
            .into_iter()
            .next()
            .expect("Length is check above"))
    }

    /// Verify input keyset
    ///
    /// Checks that the inputs are all of the same unit
    ///
    /// # Panics
    ///
    /// This function will not panic as the iterator is guaranteed to have exactly one element
    /// when it reaches the expect call (length is checked above).
    #[instrument(skip_all)]
    pub async fn verify_inputs_keyset(&self, inputs: &Proofs) -> Result<CurrencyUnit, Error> {
        let mut keyset_units = HashSet::new();

        let inputs_keyset_ids: HashSet<Id> = inputs.iter().map(|p| p.keyset_id).collect();

        for id in &inputs_keyset_ids {
            match self.get_keyset_info(id) {
                Some(keyset) => {
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

        Ok(keyset_units
            .into_iter()
            .next()
            .expect("Length is check above"))
    }

    /// Verifies that the outputs have not already been signed
    #[instrument(skip_all)]
    pub async fn check_output_already_signed(
        &self,
        tx: &mut Box<dyn cdk_database::MintTransaction<cdk_database::Error> + Send + Sync>,
        outputs: &[BlindedMessage],
    ) -> Result<(), Error> {
        let blinded_messages: Vec<PublicKey> = outputs.iter().map(|o| o.blinded_secret).collect();

        if tx
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::debug!("Transaction attempted where output is already signed.");

            return Err(Error::BlindedMessageAlreadySigned);
        }

        Ok(())
    }

    /// Verifies outputs
    ///
    /// Checks outputs are unique, of the same unit and not signed before.
    /// Returns an error if outputs are empty - callers should guard against
    /// empty outputs before calling this function.
    #[instrument(skip_all)]
    pub async fn verify_outputs(
        &self,
        tx: &mut Box<dyn cdk_database::MintTransaction<cdk_database::Error> + Send + Sync>,
        outputs: &[BlindedMessage],
    ) -> Result<Verification, Error> {
        if outputs.is_empty() {
            tracing::debug!("verify_outputs called with empty outputs");
            return Err(Error::TransactionUnbalanced(0, 0, 0));
        }

        Mint::check_outputs_unique(outputs)?;
        self.check_output_already_signed(tx, outputs).await?;

        let unit = self.verify_outputs_keyset(outputs)?;

        let amount = Amount::try_sum(outputs.iter().map(|o| o.amount))?.with_unit(unit);

        Ok(Verification { amount })
    }

    /// Verifies inputs
    ///
    /// Checks that inputs are unique and of the same unit.
    /// **NOTE: This does not check if inputs have been spent
    #[instrument(skip_all)]
    pub async fn verify_inputs(&self, inputs: &Proofs) -> Result<Verification, Error> {
        Mint::check_inputs_unique(inputs)?;
        let unit = self.verify_inputs_keyset(inputs).await?;
        let amount = inputs.total_amount()?.with_unit(unit);

        self.verify_proofs(inputs.clone()).await?;

        Ok(Verification { amount })
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

        // Units are now embedded in the typed amounts - check they match
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
}
