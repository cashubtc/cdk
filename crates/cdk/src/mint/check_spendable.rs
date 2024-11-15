use std::collections::HashSet;

use tracing::instrument;

use super::{CheckStateRequest, CheckStateResponse, Mint, ProofState, PublicKey, State};
use crate::Error;

impl Mint {
    /// Check state
    #[instrument(skip_all)]
    pub async fn check_state(
        &self,
        check_state: &CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        let states = self.localstore.get_proofs_states(&check_state.ys).await?;

        let states = states
            .iter()
            .zip(&check_state.ys)
            .map(|(state, y)| {
                let state = match state {
                    Some(state) => *state,
                    None => State::Unspent,
                };

                ProofState {
                    y: *y,
                    state,
                    witness: None,
                }
            })
            .collect();

        Ok(CheckStateResponse { states })
    }

    /// Check Tokens are not spent or pending
    #[instrument(skip_all)]
    pub async fn check_ys_spendable(
        &self,
        ys: &[PublicKey],
        proof_state: State,
    ) -> Result<(), Error> {
        let proofs_state = self
            .localstore
            .update_proofs_states(ys, proof_state)
            .await?;

        let proofs_state = proofs_state.iter().flatten().collect::<HashSet<&State>>();

        if proofs_state.contains(&State::Pending) {
            return Err(Error::TokenPending);
        }

        if proofs_state.contains(&State::Spent) {
            return Err(Error::TokenAlreadySpent);
        }

        for public_key in ys {
            self.pubsub_manager.proof_state((*public_key, proof_state));
        }

        Ok(())
    }
}
