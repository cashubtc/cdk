//! Spendable Nullifiers Checks

use cdk_common::{
    kvac::{KvacCheckStateRequest, KvacCheckStateResponse, KvacCoinState},
    State,
};

use crate::{Error, Mint};

impl Mint {
    /// Returns the state of the requested nullifiers
    pub async fn kvac_check_state(
        &self,
        check_state: &KvacCheckStateRequest,
    ) -> Result<KvacCheckStateResponse, Error> {
        tracing::info!("KVAC checkstate has been called");
        let states = self
            .localstore
            .get_kvac_nullifiers_states(&check_state.nullifiers)
            .await?;

        let states = states
            .iter()
            .zip(&check_state.nullifiers)
            .map(|(state, nullifier)| {
                let state = match state {
                    Some(state) => *state,
                    None => State::Unspent,
                };

                KvacCoinState {
                    nullifier: nullifier.clone(),
                    state,
                }
            })
            .collect();

        tracing::debug!("KVAC checkstate successful!");
        Ok(KvacCheckStateResponse { states })
    }
}
