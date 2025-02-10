use std::collections::HashSet;

use cashu_kvac::secp::GroupElement;
use cdk_common::kvac::KvacNullifier;
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
            tracing::debug!("proof: {} set to {}", public_key.to_hex(), proof_state);
            self.pubsub_manager.proof_state((*public_key, proof_state));
        }

        Ok(())
    }

    /// Check KVAC nullifiers are not spent or pending
    pub async fn check_nullifiers_spendable(
        &self,
        nullifiers: &[KvacNullifier],
        state: State,
    ) -> Result<(), Error> {
        let nullifiers_inner = nullifiers
            .iter()
            .map(|n| n.nullifier.clone())
            .collect::<Vec<GroupElement>>();

        let nullifiers_states = self
            .localstore
            .update_kvac_nullifiers_states(&nullifiers_inner, state)
            .await?;

        let nullifiers_states = nullifiers_states
            .iter()
            .flatten()
            .collect::<HashSet<&State>>();

        if nullifiers_states.contains(&State::Pending) {
            return Err(Error::TokenPending);
        }

        if nullifiers_states.contains(&State::Spent) {
            return Err(Error::TokenAlreadySpent);
        }

        Ok(())
    }
}
