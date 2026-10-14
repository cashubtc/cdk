use cdk_common::nuts::nut03::tweak_swap_outputs;
use cdk_common::SpendingConditionVerification;
use swap_saga::SwapSaga;
use tracing::instrument;

use super::{Mint, SwapRequest, SwapResponse};
use crate::Error;

pub mod swap_saga;

#[cfg(test)]
mod tests;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = super::MintMetricGuard::new("process_swap_request");

        let result = async {
            swap_request.input_amount()?;
            swap_request.output_amount()?;

            let input_proofs = swap_request.inputs();

            if input_proofs.is_empty() {
                return Err(Error::TransactionUnbalanced(
                    0,
                    swap_request.output_amount()?.to_u64(),
                    0,
                ));
            }

            // Verify inputs (cryptographic verification, no DB needed)
            let input_verification = self.verify_inputs(input_proofs).await.map_err(|err| {
                tracing::debug!("Input verification failed: {:?}", err);
                err
            })?;

            // Verify spending conditions (NUT-10/NUT-11/NUT-14), i.e. P2PK
            // and HTLC (including SIGALL)
            swap_request.verify_spending_conditions()?;

            // Prepare the execution points while R is private. The next slice will
            // reserve these points in TX1 and persist R with the signatures in TX2.
            // Until then do not enter the saga or disclose R to the client.
            if swap_request.is_unpoisonable() {
                Self::check_outputs_unique(swap_request.outputs())?;

                // Retrying cannot fix duplicate originals, but can fix a rare
                // collision between distinct derived points. Even one collision
                // is extraordinarily unlikely (see the collision discussion at
                // https://gist.github.com/SatsAndSports/6c197ca9da984c1922483e74281d38db).
                // The limit bounds retries if something is unexpectedly broken.
                let mut prepared = false;
                for _ in 0..16 {
                    let mut r = [0u8; 32];
                    getrandom::fill(&mut r).map_err(|_| Error::Internal)?;
                    let tweaked_outputs = tweak_swap_outputs(&r, swap_request.outputs())?;
                    if Self::check_outputs_unique(&tweaked_outputs).is_ok() {
                        prepared = true;
                        break;
                    }
                }
                if !prepared {
                    return Err(Error::Internal);
                }
                return Err(Error::Custom(
                    "Unpoisonable swap execution is not implemented".to_owned(),
                ));
            }

            // Step 1: Initialize the swap saga
            let init_saga =
                SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());

            // Step 2: TX1 - Setup swap (verify balance + add inputs as pending + add output blinded messages)
            let setup_saga = init_saga
                .setup_swap(
                    swap_request.inputs(),
                    swap_request.outputs(),
                    None,
                    input_verification,
                )
                .await?;

            // Step 3: Blind sign outputs (no DB transaction)
            let signed_saga = setup_saga.sign_outputs().await?;

            // Step 4: TX2 - Finalize swap (add signatures + mark inputs spent)
            let response = signed_saga.finalize().await?;

            Ok(response)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }
}
