use cdk_common::SpendingConditionVerification;
#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
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
        METRICS.inc_in_flight_requests("process_swap_request");

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

        // Check max outputs limit
        let outputs_count = swap_request.outputs().len();
        if outputs_count > self.max_outputs {
            tracing::warn!(
                "Swap request exceeds max outputs limit: {} > {}",
                outputs_count,
                self.max_outputs
            );
            return Err(Error::MaxOutputsExceeded {
                actual: outputs_count,
                max: self.max_outputs,
            });
        }

        // We don't need to check P2PK or HTLC again. It has all been checked above
        // and the code doesn't reach here unless such verifications were satisfactory

        // NUT-CTF trading: conditional tokens may be refreshed/transferred via
        // regular NUT-03 swap only when every input and output belongs to the
        // same condition outcome collection. Conditional -> regular remains
        // the oracle-witness redemption path (`POST /v1/redeem_outcome`).
        #[cfg(feature = "conditional-tokens")]
        {
            let mut conditional_input: Option<(String, String)> = None;
            let mut saw_regular_input = false;

            for proof in input_proofs {
                match self
                    .localstore
                    .get_condition_for_keyset(&proof.keyset_id)
                    .await?
                {
                    Some((condition_id, _outcome_collection, outcome_collection_id)) => {
                        let current = (condition_id, outcome_collection_id);
                        if conditional_input
                            .as_ref()
                            .is_some_and(|expected| expected != &current)
                        {
                            return Err(Error::InputsMustUseSameConditionalKeyset);
                        }
                        conditional_input = Some(current);
                    }
                    None => saw_regular_input = true,
                }
            }

            if let Some(expected) = conditional_input {
                if saw_regular_input {
                    return Err(Error::InputsMustUseSameConditionalKeyset);
                }

                for output in swap_request.outputs() {
                    match self
                        .localstore
                        .get_condition_for_keyset(&output.keyset_id)
                        .await?
                    {
                        Some((ref condition_id, _outcome_collection, ref outcome_collection_id))
                            if condition_id == &expected.0
                                && outcome_collection_id == &expected.1 => {}
                        _ => return Err(Error::InputsMustUseSameConditionalKeyset),
                    }
                }
            }
        }

        // Verify inputs (cryptographic verification, no DB needed)
        let input_verification = self.verify_inputs(input_proofs).await.map_err(|err| {
            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");

            tracing::debug!("Input verification failed: {:?}", err);
            err
        })?;

        // Verify spending conditions (NUT-10/NUT-11/NUT-14), i.e. P2PK
        // and HTLC (including SIGALL)
        swap_request.verify_spending_conditions()?;

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

    #[cfg(feature = "prometheus")]
    fn record_swap_failure(&self, operation: &str) {
        METRICS.dec_in_flight_requests(operation);
        METRICS.record_mint_operation(operation, false);
        METRICS.record_error();
    }
}
