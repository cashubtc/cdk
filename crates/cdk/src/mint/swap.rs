use tracing::instrument;

use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::{Mint, PublicKey, SigFlag, State, SwapRequest, SwapResponse};
use crate::nuts::nut00::ProofsMethods;
use crate::{cdk_database, Error};

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let mut tx = self.localstore.begin_transaction().await?;

        if let Err(err) = self
            .verify_transaction_balanced(&mut tx, swap_request.inputs(), swap_request.outputs())
            .await
        {
            tracing::debug!("Attempt to swap unbalanced transaction, aborting: {err}");
            return Err(err);
        };

        self.validate_sig_flag(&swap_request).await?;

        // After swap request is fully validated, add the new proofs to DB
        let input_ys = swap_request.inputs().ys()?;
        if let Some(err) = tx
            .add_proofs(swap_request.inputs().clone(), None)
            .await
            .err()
        {
            return match err {
                cdk_common::database::Error::Duplicate => Err(Error::TokenPending),
                cdk_common::database::Error::AttemptUpdateSpentProof => {
                    Err(Error::TokenAlreadySpent)
                }
                err => Err(Error::Database(err)),
            };
        }
        self.check_ys_spendable(&mut tx, &input_ys, State::Pending)
            .await?;

        let mut promises = Vec::with_capacity(swap_request.outputs().len());

        for blinded_message in swap_request.outputs() {
            let blinded_signature = self.blind_sign(blinded_message.clone()).await?;
            promises.push(blinded_signature);
        }

        tx.update_proofs_states(&input_ys, State::Spent)
            .await
            .map_err(|e| match e {
                cdk_database::Error::AttemptUpdateSpentProof => Error::TokenAlreadySpent,
                e => e.into(),
            })?;

        for pub_key in input_ys {
            self.pubsub_manager.proof_state((pub_key, State::Spent));
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

        tx.commit().await?;

        Ok(SwapResponse::new(promises))
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
