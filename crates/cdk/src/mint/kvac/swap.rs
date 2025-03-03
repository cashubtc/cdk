use cashu_kvac::secp::GroupElement;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::{KvacIssuedMac, KvacNullifier, KvacSwapRequest, KvacSwapResponse};
use cdk_common::State;
use tracing::instrument;

use crate::{Error, Mint};

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_kvac_swap_request(
        &self,
        swap_request: KvacSwapRequest,
    ) -> Result<KvacSwapResponse, Error> {
        tracing::info!("KVAC swap has been called");

        self.verify_kvac_request(
            true,
            0,
            &swap_request.inputs,
            &swap_request.outputs,
            swap_request.balance_proof,
            swap_request.mac_proofs,
            swap_request.script,
            swap_request.range_proof,
        )
        .await?;

        // Gather nullifiers
        let nullifiers = swap_request
            .inputs
            .iter()
            .map(|i| KvacNullifier::from(i).nullifier)
            .collect::<Vec<GroupElement>>();

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
                        .update_kvac_nullifiers_states(&nullifiers, State::Unspent)
                        .await?;
                    return Err(e);
                }
                Ok((mac, proof)) => {
                    issued_macs.push(KvacIssuedMac {
                        commitments: output.commitments.clone(),
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
            .update_kvac_nullifiers_states(&nullifiers, State::Spent)
            .await?;

        tracing::debug!("KVAC swap request successful");

        Ok(KvacSwapResponse {
            outputs: swap_request.outputs,
            macs: issued_macs.into_iter().map(|m| m.mac).collect(),
            proofs: iparams_proofs,
        })
    }
}
