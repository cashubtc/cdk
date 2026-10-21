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

        let proof_states = crate::mint::proofs::published_states(&self.localstore, &check_state.ys)
            .await?
            .into_iter()
            .zip(&check_state.ys)
            .map(|(state, y)| {
                state.unwrap_or(ProofState {
                    y: *y,
                    state: State::Unspent,
                    witness: None,
                    input_digest: None,
                })
            })
            .collect();

        Ok(CheckStateResponse {
            states: proof_states,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use cdk_common::mint::Operation;
    use cdk_common::nut17::NotificationId;
    use cdk_common::nuts::{CheckStateRequest, NotificationPayload, ProofsMethods};
    use cdk_common::pub_sub::Spec;
    use cdk_common::{Amount, State};

    use crate::mint::subscription::MintPubSubSpec;
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
            State::Reserved | State::PendingSpent => {
                panic!("mint proof state is not valid for check_state witness tests");
            }
        }

        let response = mint
            .check_state(&CheckStateRequest { ys: ys.clone() })
            .await
            .unwrap();

        // A NUT-17 subscriber is told the same current state.
        let spec = MintPubSubSpec::new_instance((db, Arc::new(HashMap::new())));
        let topics: Vec<_> = ys.into_iter().map(NotificationId::ProofState).collect();
        let events: Vec<_> = spec
            .get_events_from_db(&topics)
            .await
            .unwrap()
            .into_iter()
            .map(|event| match event.into_inner() {
                NotificationPayload::ProofState(state) => state,
                payload => panic!("unexpected payload: {payload:?}"),
            })
            .collect();
        assert_eq!(events, response.states);

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
    async fn test_check_state_does_not_return_witness_for_pending_pre_v3_proofs() {
        assert!(!check_state_witness_for_proof_state(State::Pending).await);
    }

    #[tokio::test]
    async fn test_check_state_returns_witness_for_spent_proofs() {
        assert!(check_state_witness_for_proof_state(State::Spent).await);
    }

    #[tokio::test]
    async fn test_check_state_returns_spent_for_duplicate_y() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(100)).await.unwrap();
        let ys = proofs.ys().unwrap();
        let duplicated_y = ys[0];
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

        {
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

        let response = mint
            .check_state(&CheckStateRequest {
                ys: vec![duplicated_y, duplicated_y],
            })
            .await
            .unwrap();

        assert_eq!(response.states.len(), 2);
        assert!(response
            .states
            .iter()
            .all(|proof_state| proof_state.y == duplicated_y && proof_state.state == State::Spent));
    }
}
