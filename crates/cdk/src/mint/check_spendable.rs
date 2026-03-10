use std::collections::HashMap;

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
        // Check max inputs limit
        let ys_count = check_state.ys.len();
        if ys_count > self.max_inputs {
            tracing::warn!(
                "CheckState request exceeds max inputs limit: {} > {}",
                ys_count,
                self.max_inputs
            );
            return Err(Error::MaxInputsExceeded {
                actual: ys_count,
                max: self.max_inputs,
            });
        }

        let states = self.localstore.get_proofs_states(&check_state.ys).await?;

        if check_state.ys.len() != states.len() {
            tracing::error!("Database did not return states for all proofs");
            return Err(Error::UnknownPaymentState);
        }

        // Collect ys that need witness fetching (where state.is_some())
        let ys_needing_witness: Vec<_> = check_state
            .ys
            .iter()
            .zip(states.iter())
            .filter_map(|(y, state)| state.as_ref().map(|_| *y))
            .collect();

        // Build a lookup map for witnesses (only query if there are ys to fetch)
        let witness_map: HashMap<_, _> = if ys_needing_witness.is_empty() {
            HashMap::new()
        } else {
            self.localstore
                .get_proofs_by_ys(&ys_needing_witness)
                .await?
                .into_iter()
                .flatten()
                .filter_map(|p| p.y().ok().map(|y| (y, p.witness)))
                .collect()
        };

        // Construct response without additional queries
        let proof_states = check_state
            .ys
            .iter()
            .zip(states.iter())
            .map(|(y, state)| ProofState {
                y: *y,
                state: state.unwrap_or(State::Unspent),
                witness: witness_map.get(y).cloned().flatten(),
            })
            .collect();

        Ok(CheckStateResponse {
            states: proof_states,
        })
    }
}
