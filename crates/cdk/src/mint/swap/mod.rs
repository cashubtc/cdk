use std::sync::Arc;

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
    ///
    /// The swap runs in a detached task so it completes even if the caller's
    /// future is dropped (for example, when an HTTP client disconnects).
    /// Abandoning it between the setup and finalize transactions would leave
    /// the inputs pending until the next mint restart.
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        self: &Arc<Self>,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let mint = Arc::clone(self);
        let task = tokio::spawn(tracing::Instrument::in_current_span(async move {
            mint.run_swap(swap_request).await
        }));

        match task.await {
            Ok(result) => result,
            Err(err) => match err.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(err) => {
                    tracing::error!("Swap task failed to join: {}", err);
                    Err(Error::Internal)
                }
            },
        }
    }

    async fn run_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
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
