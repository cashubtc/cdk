use std::collections::HashSet;

use cashu_kvac::secp::GroupElement;
use cdk_common::kvac::KvacNullifier;
use tracing::instrument;

use super::{CheckStateRequest, CheckStateResponse, Mint, ProofState, PublicKey, State};
use crate::{cdk_database, Error};

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
        let original_proofs_state =
            match self.localstore.update_proofs_states(ys, proof_state).await {
                Ok(states) => states,
                Err(cdk_database::Error::AttemptUpdateSpentProof)
                | Err(cdk_database::Error::AttemptRemoveSpentProof) => {
                    return Err(Error::TokenAlreadySpent)
                }
                Err(err) => return Err(err.into()),
            };

        let proofs_state = original_proofs_state
            .iter()
            .flatten()
            .collect::<HashSet<&State>>();

        if proofs_state.contains(&State::Pending) {
            // Reset states before returning error
            for (y, state) in ys.iter().zip(original_proofs_state.iter()) {
                if let Some(original_state) = state {
                    self.localstore
                        .update_proofs_states(&[*y], *original_state)
                        .await?;
                }
            }
            return Err(Error::TokenPending);
        }

        if proofs_state.contains(&State::Spent) {
            // Reset states before returning error
            for (y, state) in ys.iter().zip(original_proofs_state.iter()) {
                if let Some(original_state) = state {
                    self.localstore
                        .update_proofs_states(&[*y], *original_state)
                        .await?;
                }
            }
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

        let original_nullifiers_states = self
            .localstore
            .update_kvac_nullifiers_states(&nullifiers_inner, state)
            .await?;

        let nullifiers_states = original_nullifiers_states
            .iter()
            .flatten()
            .collect::<HashSet<&State>>();

        if nullifiers_states.contains(&State::Pending) {
            // Reset states before returning error
            for (n, state) in nullifiers.iter().zip(original_nullifiers_states.iter()) {
                if let Some(original_state) = state {
                    self.localstore
                        .update_kvac_nullifiers_states(&[n.nullifier.clone()], *original_state)
                        .await?;
                }
            }
            return Err(Error::TokenPending);
        }

        if nullifiers_states.contains(&State::Spent) {
            // Reset states before returning error
            for (n, state) in nullifiers.iter().zip(original_nullifiers_states.iter()) {
                if let Some(original_state) = state {
                    self.localstore
                        .update_kvac_nullifiers_states(&[n.nullifier.clone()], *original_state)
                        .await?;
                }
            }
            return Err(Error::TokenAlreadySpent);
        }

        Ok(())
    }
}
