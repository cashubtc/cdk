use std::borrow::Cow;

use cdk_common::nuts::nut03::{self, tweak_swap_outputs, UnpoisonableR};
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
        self.process_swap_with_randomness(swap_request, || {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|_| Error::Internal)?;
            Ok(bytes)
        })
        .await
    }

    // Injectable randomness keeps collision retries testable without weakening the public API.
    async fn process_swap_with_randomness<F>(
        &self,
        swap_request: SwapRequest,
        mut random: F,
    ) -> Result<SwapResponse, Error>
    where
        F: FnMut() -> Result<[u8; 32], Error>,
    {
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

            if swap_request.is_unpoisonable() {
                // Reject duplicate originals before retrying tweaked output reservations.
                self.verify_outputs(swap_request.outputs())?;
            }
            // Even one collision is extraordinarily unlikely; bound retries defensively.
            // https://gist.github.com/SatsAndSports/6c197ca9da984c1922483e74281d38db
            for _ in 0..16 {
                let r = if swap_request.is_unpoisonable() {
                    Some(UnpoisonableR(random()?))
                } else {
                    None
                };
                let outputs = match r {
                    Some(r) => match tweak_swap_outputs(&r.0, swap_request.outputs()) {
                        Ok(outputs) => Cow::Owned(outputs),
                        Err(nut03::Error::UnpoisonablePointAtInfinity) => continue,
                        Err(err) => return Err(err.into()),
                    },
                    None => Cow::Borrowed(swap_request.outputs().as_slice()),
                };
                let init_saga =
                    SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone())
                        .with_unpoisonable_r(r);
                let setup = init_saga
                    .setup_swap(
                        swap_request.inputs(),
                        &outputs,
                        None,
                        input_verification.clone(),
                    )
                    .await;
                let setup_saga = match setup {
                    // setup_swap rolls back TX1 on an output reservation conflict.
                    Err(Error::DuplicateOutputs) if r.is_some() => continue,
                    other => other?,
                };
                return setup_saga.sign_outputs().await?.finalize().await;
            }
            Err(Error::Internal)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }
}
