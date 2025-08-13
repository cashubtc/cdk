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
        // Do the external call before beginning the db transaction
        // Check any overflow before talking to the signatory
        swap_request.input_amount()?;
        swap_request.output_amount()?;

        let promises = self.blind_sign(swap_request.outputs().to_owned()).await?;
        let input_verification =
            self.verify_inputs(swap_request.inputs())
                .await
                .map_err(|err| {
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
            return Err(err);
        };

        self.validate_sig_flag(&swap_request).await?;

        let mut proof_writer =
            ProofWriter::new(self.localstore.clone(), self.pubsub_manager.clone());
        let input_ys = proof_writer
            .add_proofs(&mut tx, swap_request.inputs())
            .await?;

        proof_writer
            .update_proofs_states(&mut tx, &input_ys, State::Spent)
            .await?;

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

        Ok(SwapResponse::new(promises))
    }

    async fn validate_sig_flag(&self, swap_request: &SwapRequest) -> Result<(), Error> {
        let EnforceSigFlag { sig_flag, .. } = enforce_sig_flag(swap_request.inputs().clone());

        if sig_flag == SigFlag::SigAll {
            swap_request.verify_sig_all()?;
        }

        Ok(())
    }
}
