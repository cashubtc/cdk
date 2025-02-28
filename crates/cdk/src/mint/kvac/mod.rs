use std::collections::HashSet;

use crate::Error;
use cashu_kvac::{
    kvac::{BalanceProof, RangeProof},
    models::{RandomizedCoin, RangeZKP, ZKP},
    secp::{GroupElement, Scalar},
    transcript::CashuTranscript,
};
use cdk_common::{
    kvac::{KvacCoinMessage, KvacNullifier, KvacRandomizedCoin},
    Id, State,
};

use super::Mint;

mod bootstrap;
mod check_spendable;
mod melt;
mod mint;
mod restore;
mod swap;

impl Mint {
    /// Unified processing of a generic KVAC request
    pub async fn verify_kvac_request(
        &self,
        apply_fee: bool,
        delta: i64,
        inputs: &Vec<KvacRandomizedCoin>,
        outputs: &Vec<KvacCoinMessage>,
        balance_proof: ZKP,
        mac_proofs: Vec<ZKP>,
        script: Option<String>,
        range_proof: RangeZKP,
    ) -> Result<(), Error> {
        let inputs_len = inputs.len();

        if outputs.len() != 2 {
            return Err(Error::RequestInvalidOutputLength);
        }
        if inputs_len < 2 {
            return Err(Error::RequestInvalidInputLength);
        }

        let outputs_tags: Vec<Scalar> = outputs.iter().map(|output| output.t_tag.clone()).collect();

        if self
            .localstore
            .get_kvac_issued_macs_by_tags(&outputs_tags)
            .await?
            .first()
            .is_some()
        {
            tracing::error!("Outputs have already been issued a MAC",);

            return Err(Error::MacAlreadyIssued);
        }

        let fee = if apply_fee {
            i64::try_from(self.get_kvac_inputs_fee(inputs).await?)?
        } else {
            0
        };

        // Verify Balance Proof with fee as the difference amount
        let input_coins = inputs
            .iter()
            .map(|i| i.randomized_coin.clone())
            .collect::<Vec<RandomizedCoin>>();
        let output_coins = outputs
            .iter()
            .map(|i| i.commitments.0.clone())
            .collect::<Vec<GroupElement>>();
        let mut verify_transcript = CashuTranscript::new();
        if !BalanceProof::verify(
            &input_coins,
            &output_coins,
            fee + delta,
            balance_proof,
            &mut verify_transcript,
        ) {
            tracing::error!("Request is unbalanced for fee {} and delta {}", fee, delta);

            return Err(Error::BalanceVerificationError(delta, fee));
        }

        let nullifiers = inputs
            .iter()
            .map(KvacNullifier::from)
            .collect::<Vec<KvacNullifier>>();
        self.localstore.add_kvac_nullifiers(&nullifiers).await?;
        self.check_nullifiers_spendable(&nullifiers, State::Pending)
            .await?;

        // Check that there are no duplicate proofs in request
        let nullifiers_inner = nullifiers
            .iter()
            .map(|n| n.nullifier.clone())
            .collect::<Vec<GroupElement>>();
        if nullifiers_inner
            .iter()
            .collect::<HashSet<&GroupElement>>()
            .len()
            .ne(&inputs_len)
        {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::DuplicateInputs);
        }

        // Extract script if present
        let script = script.unwrap_or_default();

        // Check the MAC proofs for valid MAC issuance on the inputs
        if inputs.len() != mac_proofs.len() {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::InputsToProofsLengthMismatch);
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
        let amount_commitments = outputs
            .iter()
            .map(|o| o.commitments.0.clone())
            .collect::<Vec<GroupElement>>();
        if !RangeProof::verify(&mut verify_transcript, &amount_commitments, range_proof) {
            tracing::error!("Range proof failed to verify");
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::RangeProofVerificationError);
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

        // TODO: Script validation and execution
        Ok(())
    }
}
