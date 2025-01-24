use std::collections::HashSet;
use std::mem::swap;

use cashu_kvac::kvac::BalanceProof;
use cashu_kvac::models::RandomizedCoin;
use cashu_kvac::secp::GroupElement;
use cashu_kvac::secp::Scalar;
use cashu_kvac::transcript::CashuTranscript;
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
        let inputs_len = swap_request.inputs.len();

        if swap_request.outputs.len() != 2 {
            return Err(Error::RequestInvalidOutputLength)
        }
        if inputs_len < 2 {
            return Err(Error::RequestInvalidInputLength)
        }

        let outputs_tags: Vec<Scalar> = swap_request.outputs
            .iter()
            .map(|output| output.t_tag)
            .collect();

        if self
            .localstore
            .get_kvac_issued_macs_by_tags(&outputs_tags)
            .await?
            .iter()
            .flatten()
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
        if nullifiers
            .iter()
            .map(|n| &n.nullifier)
            .collect::<HashSet<&GroupElement>>()
            .len()
            .ne(&inputs_len)
        {
            self.localstore
                .update_kvac_nullifiers_states(&nullifiers, State::Unspent)
                .await?;
            return Err(Error::DuplicateProofs);
        }

        Ok(())
    }
}