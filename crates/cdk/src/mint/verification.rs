use std::collections::HashSet;

use cdk_common::{Amount, BlindedMessage, CurrencyUnit, Id, Proofs, ProofsMethods, PublicKey};

use super::{Error, Mint};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Verification {
    pub amount: Amount,
    pub unit: CurrencyUnit,
}

impl Mint {
    /// Verify that the inputs to the transaction are unique
    pub fn check_inputs_unique(inputs: &Proofs) -> Result<(), Error> {
        let proof_count = inputs.len();

        if inputs
            .iter()
            .map(|i| i.y())
            .collect::<Result<HashSet<PublicKey>, _>>()?
            .len()
            .ne(&proof_count)
        {
            return Err(Error::DuplicateInputs);
        }

        Ok(())
    }

    /// Verify that the outputs to are unique
    pub fn check_outputs_unique(outputs: &[BlindedMessage]) -> Result<(), Error> {
        let output_count = outputs.len();

        if outputs
            .iter()
            .map(|o| &o.blinded_secret)
            .collect::<HashSet<&PublicKey>>()
            .len()
            .ne(&output_count)
        {
            return Err(Error::DuplicateOutputs);
        }

        Ok(())
    }

    /// Verify output keyset
    ///
    /// Checks that the outputs are all of the same unit and the keyset is active
    pub async fn verify_outputs_keyset(
        &self,
        outputs: &[BlindedMessage],
    ) -> Result<CurrencyUnit, Error> {
        let mut keyset_units = HashSet::new();

        let output_keyset_ids: HashSet<Id> = outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            match self.localstore.get_keyset_info(id).await? {
                Some(keyset) => {
                    if !keyset.active {
                        return Err(Error::InactiveKeyset);
                    }
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in outputs");
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len() != 1 {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
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
    pub async fn verify_inputs_keyset(&self, inputs: &Proofs) -> Result<CurrencyUnit, Error> {
        let mut keyset_units = HashSet::new();

        let inputs_keyset_ids: HashSet<Id> = inputs.iter().map(|p| p.keyset_id).collect();

        for id in &inputs_keyset_ids {
            match self.localstore.get_keyset_info(id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in outputs");
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len() != 1 {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            return Err(Error::MultipleUnits);
        }

        Ok(keyset_units
            .into_iter()
            .next()
            .expect("Length is check above"))
    }

    /// Verifies that the outputs have not already been signed
    pub async fn check_output_already_signed(
        &self,
        outputs: &[BlindedMessage],
    ) -> Result<(), Error> {
        let blinded_messages: Vec<PublicKey> = outputs.iter().map(|o| o.blinded_secret).collect();

        if self
            .localstore
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed",);

            return Err(Error::BlindedMessageAlreadySigned);
        }

        Ok(())
    }

    /// Verifies outputs
    /// Checks outputs are unique, of the same unit and not signed before
    pub async fn verify_outputs(&self, outputs: &[BlindedMessage]) -> Result<Verification, Error> {
        Mint::check_outputs_unique(outputs)?;
        self.check_output_already_signed(outputs).await?;

        let unit = self.verify_outputs_keyset(outputs).await?;

        let amount = Amount::try_sum(outputs.iter().map(|o| o.amount).collect::<Vec<Amount>>())?;

        Ok(Verification { amount, unit })
    }

    /// Verifies inputs
    /// Checks that inputs are unique and of the same unit
    /// **NOTE: This does not check if inputs have been spent
    pub async fn verify_inputs(&self, inputs: &Proofs) -> Result<Verification, Error> {
        Mint::check_inputs_unique(inputs)?;
        let unit = self.verify_inputs_keyset(inputs).await?;
        let amount = inputs.total_amount()?;

        for proof in inputs {
            self.verify_proof(proof).await?;
        }

        Ok(Verification { amount, unit })
    }

    /// Verify that inputs and outputs are valid and balanced
    pub async fn verify_transaction_balanced(
        &self,
        inputs: &Proofs,
        outputs: &[BlindedMessage],
    ) -> Result<(), Error> {
        let output_verification = self.verify_outputs(outputs).await.map_err(|err| {
            tracing::debug!("Output verification failed: {:?}", err);
            err
        })?;
        let input_verification = self.verify_inputs(inputs).await.map_err(|err| {
            tracing::debug!("Input verification failed: {:?}", err);
            err
        })?;

        if output_verification.unit != input_verification.unit {
            tracing::debug!(
                "Output unit {} does not match input unit {}",
                output_verification.unit,
                input_verification.unit
            );
            return Err(Error::UnitMismatch);
        }

        let fees = self.get_proofs_fee(inputs).await?;

        if output_verification.amount
            != input_verification
                .amount
                .checked_sub(fees)
                .ok_or(Error::AmountOverflow)?
        {
            return Err(Error::TransactionUnbalanced(
                input_verification.amount.into(),
                output_verification.amount.into(),
                fees.into(),
            ));
        }

        Ok(())
    }
}
