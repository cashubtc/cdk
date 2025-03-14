//! All about verifying KVAC requests

use std::collections::HashSet;

use cashu_kvac::kvac::{BalanceProof, MacProof, RangeProof};
use cashu_kvac::models::{RandomizedCoin, RangeZKP, ZKP};
use cashu_kvac::secp::{GroupElement, Scalar};
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::Error::{
    BalanceVerificationError, InputsToProofsLengthMismatch, MacAlreadyIssued, MacVerificationError,
    RangeProofVerificationError, RequestInvalidInputLength, RequestInvalidOutputLength,
};
use cdk_common::kvac::{KvacCoinMessage, KvacNullifier, KvacRandomizedCoin};
use cdk_common::{Id, State};

use crate::{Error, Mint};

impl Mint {
    /// Checks that the outputs have not already been issued a MAC
    pub async fn verify_kvac_outputs_issuance_state(
        &self,
        outputs_tags: &[Scalar],
    ) -> Result<(), Error> {
        // Check that outputs are not already issued. USER PROTECTION.
        if !self
            .localstore
            .get_kvac_issued_macs_by_tags(outputs_tags)
            .await?
            .is_empty()
        {
            tracing::error!("Outputs have already been issued a MAC",);
            return Err(Error::from(MacAlreadyIssued));
        }

        Ok(())
    }

    /// Checks the provided balance proof with inputs, outputs and delta amount
    pub fn verify_kvac_inputs_outputs_balanced(
        &self,
        inputs: &[KvacRandomizedCoin],
        outputs: &[KvacCoinMessage],
        delta: i64,
        fee: i64,
        balance_proof: ZKP,
        transcript: &mut CashuTranscript,
    ) -> Result<(), Error> {
        let input_commitments = inputs
            .iter()
            .map(|i| i.randomized_coin.clone())
            .collect::<Vec<RandomizedCoin>>();
        let output_commitments = outputs
            .iter()
            .map(|i| i.commitments.0.clone())
            .collect::<Vec<GroupElement>>();
        if !BalanceProof::verify(
            &input_commitments,
            &output_commitments,
            fee + delta,
            balance_proof,
            transcript,
        ) {
            tracing::error!("Request is unbalanced for fee {} and delta {}", fee, delta);

            return Err(Error::from(BalanceVerificationError(delta, fee)));
        }

        Ok(())
    }

    /// Checks no duplicate inputs where provided
    pub fn check_no_kvac_duplicate_inputs(
        &self,
        nullifiers_inner: &[GroupElement],
    ) -> Result<(), Error> {
        if nullifiers_inner
            .iter()
            .collect::<HashSet<&GroupElement>>()
            .len()
            .ne(&nullifiers_inner.len())
        {
            return Err(Error::DuplicateInputs);
        }

        Ok(())
    }

    /// Verify [`MAC`]
    pub async fn verify_mac(
        &self,
        input: &KvacRandomizedCoin,
        script: &String,
        proof: ZKP,
        verifying_transcript: &mut CashuTranscript,
    ) -> Result<(), Error> {
        self.ensure_kvac_keyset_loaded(&input.keyset_id).await?;

        let keysets = &self.kvac_keysets.read().await;
        let keyset = keysets.get(&input.keyset_id).ok_or(Error::UnknownKeySet)?;

        let private_key = &keyset.kvac_keys.private_key;

        if !MacProof::verify(
            private_key,
            &input.randomized_coin,
            Some(script.as_bytes()),
            proof,
            verifying_transcript,
        ) {
            return Err(Error::from(MacVerificationError));
        }

        Ok(())
    }

    /// Check that the outputs are within a certain amount each
    pub fn verify_kvac_outputs_in_range(
        &self,
        outputs: &[KvacCoinMessage],
        range_proof: RangeZKP,
        transcript: &mut CashuTranscript,
    ) -> Result<(), Error> {
        let amount_commitments = outputs
            .iter()
            .map(|o| o.commitments.0.clone())
            .collect::<Vec<GroupElement>>();
        if !RangeProof::verify(transcript, &amount_commitments, range_proof) {
            tracing::error!("Range proof failed to verify");
            return Err(Error::from(RangeProofVerificationError));
        }
        Ok(())
    }

    /// Unified processing of a generic KVAC request
    #[allow(clippy::too_many_arguments)]
    pub async fn verify_kvac_request(
        &self,
        apply_fee: bool,
        delta: i64,
        inputs: &[KvacRandomizedCoin],
        outputs: &[KvacCoinMessage],
        balance_proof: ZKP,
        mac_proofs: Vec<ZKP>,
        script: Option<String>,
        range_proof: RangeZKP,
    ) -> Result<(), Error> {
        let inputs_len = inputs.len();

        // Inputs/outputs length constraints requirements
        if outputs.len() != 2 {
            return Err(Error::from(RequestInvalidOutputLength));
        }
        if inputs_len < 2 {
            return Err(Error::from(RequestInvalidInputLength));
        }

        // Extract identifiers for the outputs
        let outputs_tags: Vec<Scalar> = outputs.iter().map(|output| output.t_tag.clone()).collect();

        // Verify the state of issuance of the outputs
        self.verify_kvac_outputs_issuance_state(&outputs_tags)
            .await?;

        // Do we need to apply a fee to this request?
        let fee = if apply_fee {
            i64::try_from(self.get_kvac_inputs_fee(inputs).await?)?
        } else {
            0
        };

        // Instantiate verification transcript
        let mut verify_transcript = CashuTranscript::new();

        // Verify balance proof with fee as the difference amount
        self.verify_kvac_inputs_outputs_balanced(
            inputs,
            outputs,
            delta,
            fee,
            balance_proof,
            &mut verify_transcript,
        )?;

        // Extract nullifiers
        let nullifiers = inputs
            .iter()
            .map(KvacNullifier::from)
            .collect::<Vec<KvacNullifier>>();

        // Add the nullifiers to DB. From this point on every failure has to be followed
        // by a reset of the states of these nullifiers
        self.localstore.add_kvac_nullifiers(&nullifiers).await?;
        self.check_nullifiers_spendable(&nullifiers, State::Pending)
            .await?;

        let nullifiers_inner = nullifiers
            .iter()
            .map(|n| n.nullifier.clone())
            .collect::<Vec<GroupElement>>();

        // Check that there are no duplicate proofs in request
        let ok = self.check_no_kvac_duplicate_inputs(&nullifiers_inner);
        if let Err(e) = ok {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(e);
        }

        // Extract script if present
        let script = script.unwrap_or_default();
        // TODO: Script validation is not yet implemented.

        // Check the MAC proofs for valid MAC issuance on the inputs
        if inputs.len() != mac_proofs.len() {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::from(InputsToProofsLengthMismatch));
        }
        for (input, proof) in inputs.iter().zip(mac_proofs.into_iter()) {
            let result = self
                .verify_mac(input, &script, proof, &mut verify_transcript)
                .await;
            if let Err(e) = result {
                tracing::error!("MAC verification failure");
                self.localstore
                    .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                    .await?;
                return Err(e);
            }
        }

        // Debug: print the state of the transcript
        //let test = verify_transcript.get_challenge(b"test");
        //tracing::debug!("test challenge: {}", String::from(&test));

        // Verify the outputs are within range
        let ok = self.verify_kvac_outputs_in_range(outputs, range_proof, &mut verify_transcript);
        if let Err(e) = ok {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(e);
        }

        let input_keyset_ids: HashSet<Id> = inputs.iter().map(|p| p.keyset_id).collect();
        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            match self.localstore.get_kvac_keyset_info(&id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::error!("Request with unknown keyset in inputs");
                    self.localstore
                        .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                        .await?;
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        let output_keyset_ids: HashSet<Id> = outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            match self.localstore.get_kvac_keyset_info(id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::error!("Request with unknown keyset in outputs");
                    self.localstore
                        .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                        .await?;
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::MultipleUnits);
        }

        Ok(())
    }
}
