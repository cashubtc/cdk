use tracing::instrument;

use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::{Mint, PublicKey, SigFlag, State, SwapRequest, SwapResponse};
use crate::nuts::nut00::ProofsMethods;
use crate::Error;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let input_ys = swap_request.inputs.ys()?;

        if let Err(err) = self
            .verify_transaction_balanced(&swap_request.inputs, &swap_request.outputs)
            .await
        {
            tracing::debug!("Attempt to swap unbalanced transaction: {}", err);
            self.localstore.remove_proofs(&input_ys, None).await?;
            return Err(err);
        };

        self.localstore
            .add_proofs(swap_request.inputs.clone(), None)
            .await?;

        self.check_ys_spendable(&input_ys, State::Pending).await?;

        let EnforceSigFlag {
            sig_flag,
            pubkeys,
            sigs_required,
        } = enforce_sig_flag(swap_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_message in &swap_request.outputs {
                if let Err(err) = blinded_message.verify_p2pk(&pubkeys, sigs_required) {
                    tracing::info!("Could not verify p2pk in swap request");
                    self.localstore.remove_proofs(&input_ys, None).await?;
                    return Err(err.into());
                }
            }
        }

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for blinded_message in swap_request.outputs.iter() {
            let blinded_signature = self.blind_sign(blinded_message).await?;
            promises.push(blinded_signature);
        }

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        for pub_key in input_ys {
            self.pubsub_manager.proof_state((pub_key, State::Spent));
        }

        self.localstore
            .add_blind_signatures(
                &swap_request
                    .outputs
                    .iter()
                    .map(|o| o.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &promises,
                None,
            )
            .await?;

        Ok(SwapResponse::new(promises))
    }
}
