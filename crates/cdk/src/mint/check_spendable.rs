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

        // Collect ys that need witness fetching (only spent proofs expose witnesses)
        let ys_needing_witness: Vec<_> = check_state
            .ys
            .iter()
            .zip(states.iter())
            .filter_map(|(y, state)| match state {
                Some(State::Spent) => Some(*y),
                _ => None,
            })
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

#[cfg(test)]
mod tests {
    use cdk_common::mint::Operation;
    use cdk_common::nuts::{CheckStateRequest, ProofsMethods};
    use cdk_common::{Amount, State};

    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
    use crate::Mint;

    async fn check_state_witness_for_proof_state(state: State) -> bool {
        let mint = create_test_mint().await.unwrap();
        let mut proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();

        for proof in proofs.iter_mut() {
            proof.add_preimage("deadbeefdeadbeefdeadbeefdeadbeef".to_string());
        }

        let ys = proofs.ys().unwrap();
        let db = mint.localstore();

        {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(
                proofs,
                None,
                &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        match state {
            State::Unspent => {}
            State::Pending => {
                let mut tx = db.begin_transaction().await.unwrap();
                let mut acquired = tx.get_proofs(&ys).await.unwrap();
                Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            State::Spent => {
                let mut tx = db.begin_transaction().await.unwrap();
                let mut acquired = tx.get_proofs(&ys).await.unwrap();
                Mint::update_proofs_state(&mut tx, &mut acquired, State::Pending)
                    .await
                    .unwrap();
                Mint::update_proofs_state(&mut tx, &mut acquired, State::Spent)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            State::Reserved | State::PendingSpent | State::PendingReceive => {
                panic!("mint proof state is not valid for check_state witness tests");
            }
        }

        let response = mint.check_state(&CheckStateRequest { ys }).await.unwrap();

        assert!(response
            .states
            .iter()
            .all(|proof_state| proof_state.state == state));

        response
            .states
            .iter()
            .any(|proof_state| proof_state.witness.is_some())
    }

    #[tokio::test]
    async fn test_check_state_does_not_return_witness_for_unspent_proofs() {
        assert!(!check_state_witness_for_proof_state(State::Unspent).await);
    }

    #[tokio::test]
    async fn test_check_state_does_not_return_witness_for_pending_proofs() {
        assert!(!check_state_witness_for_proof_state(State::Pending).await);
    }

    #[tokio::test]
    async fn test_check_state_returns_witness_for_spent_proofs() {
        assert!(check_state_witness_for_proof_state(State::Spent).await);
    }
}
