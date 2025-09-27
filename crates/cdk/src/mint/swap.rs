#[cfg(feature = "prometheus")]
use cdk_prometheus::METRICS;
use tracing::instrument;

use super::blinded_message_writer::BlindedMessageWriter;
use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::proof_writer::ProofWriter;
use super::{Mint, PublicKey, SigFlag, State, SwapRequest, SwapResponse};
use crate::Error;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        #[cfg(feature = "prometheus")]
        METRICS.inc_in_flight_requests("process_swap_request");
        // Do the external call before beginning the db transaction
        // Check any overflow before talking to the signatory
        swap_request.input_amount()?;
        swap_request.output_amount()?;

        // We add blinded messages to db before attempting to sign
        // this ensures that they are unique and have not been used before
        let mut blinded_message_writer = BlindedMessageWriter::new(self.localstore.clone());
        let mut tx = self.localstore.begin_transaction().await?;

        match blinded_message_writer
            .add_blinded_messages(&mut tx, None, swap_request.outputs())
            .await
        {
            Ok(_) => {
                tx.commit().await?;
            }
            Err(err) => {
                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("process_swap_request");
                    METRICS.record_mint_operation("process_swap_request", false);
                    METRICS.record_error();
                }
                return Err(err);
            }
        }

        let promises = self.blind_sign(swap_request.outputs().to_owned()).await?;
        let input_verification =
            self.verify_inputs(swap_request.inputs())
                .await
                .map_err(|err| {
                    #[cfg(feature = "prometheus")]
                    {
                        METRICS.dec_in_flight_requests("process_swap_request");
                        METRICS.record_mint_operation("process_swap_request", false);
                        METRICS.record_error();
                    }

                    tracing::debug!("Input verification failed: {:?}", err);
                    err
                })?;
        let mut tx = self.localstore.begin_transaction().await?;

        if let Err(err) = self
            .verify_transaction_balanced(
                &mut tx,
                input_verification,
                swap_request.inputs(),
                swap_request.outputs(),
            )
            .await
        {
            tracing::debug!("Attempt to swap unbalanced transaction, aborting: {err}");

            #[cfg(feature = "prometheus")]
            {
                METRICS.dec_in_flight_requests("process_swap_request");
                METRICS.record_mint_operation("process_swap_request", false);
                METRICS.record_error();
            }

            tx.rollback().await?;
            blinded_message_writer.rollback().await?;

            return Err(err);
        };

        let validate_sig_result = self.validate_sig_flag(&swap_request).await;

        if let Err(err) = validate_sig_result {
            tx.rollback().await?;
            blinded_message_writer.rollback().await?;

            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");
            return Err(err);
        }
        let mut proof_writer =
            ProofWriter::new(self.localstore.clone(), self.pubsub_manager.clone());
        let input_ys = match proof_writer
            .add_proofs(&mut tx, swap_request.inputs(), None)
            .await
        {
            Ok(ys) => ys,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                {
                    METRICS.dec_in_flight_requests("process_swap_request");
                    METRICS.record_mint_operation("process_swap_request", false);
                    METRICS.record_error();
                }
                tx.rollback().await?;
                blinded_message_writer.rollback().await?;
                return Err(err);
            }
        };

        let update_proof_states_result = proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await;

        if let Err(err) = update_proof_states_result {
            #[cfg(feature = "prometheus")]
            self.record_swap_failure("process_swap_request");

            tx.rollback().await?;
            blinded_message_writer.rollback().await?;
            return Err(err);
        }

        tx.add_blind_signatures(
            &swap_request
                .outputs()
                .iter()
                .map(|o| o.blinded_secret)
                .collect::<Vec<PublicKey>>(),
            &promises,
            None,
        )
        .await?;

        proof_writer.commit();
        blinded_message_writer.commit();
        tx.commit().await?;

        let response = SwapResponse::new(promises);

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
