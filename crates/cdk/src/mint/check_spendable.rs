use std::collections::{HashMap, HashSet};

use tracing::instrument;

use super::{CheckStateRequest, CheckStateResponse, Mint, ProofState, PublicKey, State};
use crate::{cdk_database, Error};

impl Mint {
    /// Helper function to reset proofs to their original state, skipping spent proofs
    async fn reset_proofs_to_original_state(
        &self,
        ys: &[PublicKey],
        original_states: Vec<Option<State>>,
    ) -> Result<(), Error> {
        let mut ys_by_state = HashMap::new();
        let mut unknown_proofs = Vec::new();
        for (y, state) in ys.iter().zip(original_states) {
            if let Some(state) = state {
                // Skip attempting to update proofs that were originally spent
                if state != State::Spent {
                    ys_by_state.entry(state).or_insert_with(Vec::new).push(*y);
                }
            } else {
                unknown_proofs.push(*y);
            }
        }

        for (state, ys) in ys_by_state {
            self.localstore.update_proofs_states(&ys, state).await?;
        }

        self.localstore.remove_proofs(&unknown_proofs, None).await?;

        Ok(())
    }
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
        let original_proofs_state =
            match self.localstore.update_proofs_states(ys, proof_state).await {
                Ok(states) => states,
                Err(cdk_database::Error::AttemptUpdateSpentProof)
                | Err(cdk_database::Error::AttemptRemoveSpentProof) => {
                    return Err(Error::TokenAlreadySpent)
                }
                Err(err) => return Err(err.into()),
            };

        assert!(ys.len() == original_proofs_state.len());

        let proofs_state = original_proofs_state
            .iter()
            .flatten()
            .collect::<HashSet<&State>>();

        if proofs_state.contains(&State::Pending) {
            // Reset states before returning error
            self.reset_proofs_to_original_state(ys, original_proofs_state)
                .await?;
            return Err(Error::TokenPending);
        }

        if proofs_state.contains(&State::Spent) {
            // Reset states before returning error
            self.reset_proofs_to_original_state(ys, original_proofs_state)
                .await?;
            return Err(Error::TokenAlreadySpent);
        }

        for public_key in ys {
            tracing::trace!("proof: {} set to {}", public_key.to_hex(), proof_state);
            self.pubsub_manager.proof_state((*public_key, proof_state));
        }

        Ok(())
    }
}
