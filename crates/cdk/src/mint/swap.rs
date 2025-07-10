use tracing::instrument;

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
        self.metrics.inc_in_flight_requests("process_swap_request");

        let mut tx = self.localstore.begin_transaction().await?;

        if let Err(err) = self
            .verify_transaction_balanced(&mut tx, swap_request.inputs(), swap_request.outputs())
            .await
        {
            tracing::debug!("Attempt to swap unbalanced transaction, aborting: {err}");

            #[cfg(feature = "prometheus")]
            {
                self.metrics.dec_in_flight_requests("process_swap_request");
                self.metrics
                    .record_mint_operation("process_swap_request", false);
                self.metrics.record_error();
            }

            return Err(err);
        };

        if let Err(err) = self.validate_sig_flag(&swap_request).await {
            #[cfg(feature = "prometheus")]
            {
                self.metrics.dec_in_flight_requests("process_swap_request");
                self.metrics
                    .record_mint_operation("process_swap_request", false);
                self.metrics.record_error();
            }

            return Err(err);
        }

        let mut proof_writer =
            ProofWriter::new(self.localstore.clone(), self.pubsub_manager.clone());
        let input_ys = match proof_writer
            .add_proofs(&mut tx, swap_request.inputs())
            .await
        {
            Ok(ys) => ys,
            Err(err) => {
                #[cfg(feature = "prometheus")]
                {
                    self.metrics.dec_in_flight_requests("process_swap_request");
                    self.metrics
                        .record_mint_operation("process_swap_request", false);
                    self.metrics.record_error();
                }

                return Err(err);
            }
        };

        let mut promises = Vec::with_capacity(swap_request.outputs().len());

        for blinded_message in swap_request.outputs() {
            let blinded_signature = self.blind_sign(blinded_message.clone()).await?;
            promises.push(blinded_signature);
        }

        if let Err(err) = proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await
        {
            #[cfg(feature = "prometheus")]
            {
                self.metrics.dec_in_flight_requests("process_swap_request");
                self.metrics
                    .record_mint_operation("process_swap_request", false);
                self.metrics.record_error();
            }

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
        tx.commit().await?;

        let response = SwapResponse::new(promises);

        #[cfg(feature = "prometheus")]
        {
            self.metrics.dec_in_flight_requests("process_swap_request");
            self.metrics
                .record_mint_operation("process_swap_request", true);
        }

        Ok(response)
    }

    async fn validate_sig_flag(&self, swap_request: &SwapRequest) -> Result<(), Error> {
        let EnforceSigFlag {
            sig_flag,
            pubkeys,
            sigs_required,
        } = enforce_sig_flag(swap_request.inputs().clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_message in swap_request.outputs() {
                if let Err(err) = blinded_message.verify_p2pk(&pubkeys, sigs_required) {
                    tracing::info!("Could not verify p2pk in swap request");
                    return Err(err.into());
                }
            }
        }

        Ok(())
    }
}
