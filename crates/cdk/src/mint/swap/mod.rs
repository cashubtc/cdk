#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use swap_saga::SwapSaga;
use tracing::instrument;

use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::{Mint, SigFlag, SwapRequest, SwapResponse};
use crate::Error;

mod swap_saga;

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
        if let Err(err) = self.validate_sig_flag(&swap_request).await {
            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");
            return Err(err);
        }

        // Start the swap saga
        let mut saga = SwapSaga::new(self, self.localstore.clone(), self.pubsub_manager.clone());

        // TX1: Setup swap (verify balance + add inputs as pending + add output blinded messages)
        // The balance verification is now part of the same transaction
        if let Err(err) = saga
            .setup_swap(
                swap_request.inputs(),
                swap_request.outputs(),
                None,
                input_verification,
            )
            .await
        {
            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");
            return Err(err);
        }

        // Blind sign outputs (no DB transaction)
        if let Err(err) = saga.sign_outputs().await {
            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");
            return Err(err);
        }

        // TX2: Finalize swap (add signatures + mark inputs spent)
        let response = match saga.finalize().await {
            Ok(response) => response,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                self.record_swap_failure("process_swap_request");
                return Err(err);
            }
        };

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
