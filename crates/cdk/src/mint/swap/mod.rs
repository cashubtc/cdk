#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use swap_saga::SwapSaga;
use tracing::instrument;

use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::{Mint, SigFlag, SwapRequest, SwapResponse};
use crate::Error;

pub mod swap_saga;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_swap_request");

        swap_request.input_amount()?;
        swap_request.output_amount()?;

        // Verify inputs (cryptographic verification, no DB needed)
        let input_verification =
            self.verify_inputs(swap_request.inputs())
                .await
                .map_err(|err| {
                    #[cfg(feature = "prometheus")]
                    self.record_swap_failure("process_swap_request");

                    tracing::debug!("Input verification failed: {:?}", err);
                    err
                })?;

        // Verify signature flag (no DB needed)
        self.validate_sig_flag(&swap_request).await?;

        // Step 1: Initialize the swap saga
        let init_saga = SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());

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

        #[cfg(feature = "prometheus")]
        {
            METRICS.dec_in_flight_requests("process_swap_request");
            METRICS.record_mint_operation("process_swap_request", true);
        }

        Ok(response)
    }

    async fn validate_sig_flag(&self, swap_request: &SwapRequest) -> Result<(), Error> {
        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(swap_request.inputs().clone());

        if sig_flag == SigFlag::SigAll {
            swap_request.verify_sig_all()?;
        }

        Ok(())
    }

    #[cfg(feature = "prometheus")]
    fn record_swap_failure(&self, operation: &str) {
        METRICS.dec_in_flight_requests(operation);
        METRICS.record_mint_operation(operation, false);
        METRICS.record_error();
    }
}
