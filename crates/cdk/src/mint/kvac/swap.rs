use std::collections::HashSet;
use cashu_kvac::kvac::BalanceProof;
use cashu_kvac::kvac::RangeProof;
use cashu_kvac::models::RandomizedCoin;
use cashu_kvac::secp::GroupElement;
use cashu_kvac::secp::Scalar;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::KvacIssuedMac;
use cdk_common::kvac::KvacNullifier;
use cdk_common::kvac::{KvacSwapRequest, KvacSwapResponse};
use cdk_common::State;
use tracing::instrument;

use crate::Mint;
use crate::Error;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_kvac_swap_request(
        &self,
        swap_request: KvacSwapRequest,
    ) -> Result<KvacSwapResponse, Error> {
        tracing::debug!("KVAC swap has been called");
        let inputs_len = swap_request.inputs.len();

        if swap_request.outputs.len() != 2 {
            return Err(Error::RequestInvalidOutputLength)
        }
        if inputs_len < 2 {
            return Err(Error::RequestInvalidInputLength)
        }

        let outputs_tags: Vec<Scalar> = swap_request.outputs
            .iter()
            .map(|output| output.t_tag.clone())
            .collect();

        if self
            .localstore
            .get_kvac_issued_macs_by_tags(&outputs_tags)
            .await?
            .iter()
            .next()
            .is_some()
        {
            tracing::info!("Outputs have already been issued a MAC",);

            return Err(Error::MacAlreadyIssued);
        }

        let fee = self.get_kvac_inputs_fee(&swap_request.inputs).await?;

        // Verify Balance Proof with fee as the difference amount
        let input_coins = swap_request.inputs
            .iter()
            .map(|i| i.randomized_coin.clone())
            .collect::<Vec<RandomizedCoin>>();
        let output_coins = swap_request.outputs
            .iter()
            .map(|i| i.commitments.0.clone())
            .collect::<Vec<GroupElement>>();
        let mut verify_transcript = CashuTranscript::new();
        if !BalanceProof::verify(
            &input_coins,
            &output_coins,
            fee.0 as i64,
            swap_request.balance_proof,
            &mut verify_transcript,
        ) {
            tracing::info!("Swap request is unbalanced for fee {}", fee);

            return Err(Error::BalanceVerificationError(fee))
        }

        let nullifiers = swap_request
            .inputs
            .iter()
            .map(|i| KvacNullifier::from(i))
            .collect::<Vec<KvacNullifier>>();
        self.localstore
            .add_kvac_nullifiers(&nullifiers)
            .await?;
        self.check_nullifiers_spendable(&nullifiers, State::Pending).await?;

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
            return Err(Error::DuplicateProofs);
        }

        // Check the MAC proofs for valid MAC issuance on the inputs
        let script = swap_request.script;
        if swap_request.inputs.len() != swap_request.mac_proofs.len() {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::InputsToProofsLengthMismatch)
        }
        for (input, proof) in swap_request.inputs.iter().zip(swap_request.mac_proofs.into_iter()) {
            let result = self.verify_mac(input, &script, proof, &mut verify_transcript).await;
            if let Err(e) = result {
                self.localstore
                    .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                    .await?;
                return Err(e);
            }
        }
        
        // Verify the outputs are within range
        let commitments = swap_request.outputs
            .iter()
            .map(|o| (o.commitments.0.clone(), None))
            .collect::<Vec<(GroupElement, Option<GroupElement>)>>();
        if !RangeProof::verify(&mut verify_transcript, &commitments, swap_request.range_proof) {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                .await?;
            return Err(Error::RangeProofVerificationError)
        }

        // TODO: Script validation and execution

        // Issue MACs
        let mut issued_macs = vec![];
        let mut iparams_proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for output in swap_request.outputs.iter() {
            let result = self.issue_mac(output, &mut proving_transcript).await;
            // Set nullifiers unspent in case of error
            match result {
                Err(e) => {
                    self.localstore
                        .update_kvac_nullifiers_states(&nullifiers_inner, State::Unspent)
                        .await?;
                    return Err(e)
                },
                Ok((mac, proof)) => {
                    issued_macs.push(KvacIssuedMac {
                        mac,
                        keyset_id: output.keyset_id,
                        quote_id: None,
                    });
                    iparams_proofs.push(proof);
                }
            }
        }

        // Add issued macs
        self.localstore
            .add_kvac_issued_macs(&issued_macs, None)
            .await?;

        // Set nullifiers as spent
        self.localstore
            .update_kvac_nullifiers_states(&nullifiers_inner, State::Spent)
            .await?;

        tracing::debug!("KVAC swap request successful");
        Ok(KvacSwapResponse {
            macs: issued_macs.into_iter().map(|m| m.mac).collect(),
            proofs: iparams_proofs,
        })
    }
}