use futures::future::try_join_all;
use tracing::instrument;

use super::{CheckStateRequest, CheckStateResponse, Mint, ProofState, State};
use crate::Error;

impl Mint {
    /// Check state
    #[instrument(skip_all)]
    pub async fn check_state(
        &self,
        check_state: &CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let states = self.localstore.get_proofs_states(&check_state.ys).await?;

        if check_state.ys.len() != states.len() {
            tracing::error!("Database did not return states for all proofs");
            return Err(Error::UnknownPaymentState);
        }

        let proof_states_futures =
            check_state
                .ys
                .iter()
                .zip(states.iter())
                .map(|(y, state)| async move {
                    let witness: Result<Option<cdk_common::Witness>, Error> = if state.is_some() {
                        let proofs = self.localstore.get_proofs_by_ys(&[*y]).await?;
                        Ok(proofs.first().cloned().flatten().and_then(|p| p.witness))
                    } else {
                        Ok(None)
                    };

                    witness.map(|w| ProofState {
                        y: *y,
                        state: state.unwrap_or(State::Unspent),
                        witness: w,
                    })
                });

        let proof_states = try_join_all(proof_states_futures).await?;

        Ok(CheckStateResponse {
            states: proof_states,
        })
    }
}
