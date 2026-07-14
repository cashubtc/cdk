//! Thin persistence boundary for consensus-controlled Bitcoin chain evidence.

use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAddressAllocationState, FederationChainObservation,
    FederationChainObservationOutcome, FederationChainObservationState, FederationJournalEntry,
    FederationPaymentObservationContext,
};

use super::super::Mint;
use super::address_allocation::{
    ADDRESS_ALLOCATION_PRIMARY_NAMESPACE, ADDRESS_ALLOCATION_SECONDARY_NAMESPACE,
    ADDRESS_ALLOCATION_STATE_KEY,
};
use super::finish_federation_apply_transaction;

pub(super) const CHAIN_OBSERVATION_PRIMARY_NAMESPACE: &str = "federation";
pub(super) const CHAIN_OBSERVATION_SECONDARY_NAMESPACE: &str = "chain_observation";
pub(super) const CHAIN_OBSERVATION_STATE_KEY: &str = "state_v1";

impl Mint {
    /// Load the complete validated consensus wallet UTXO projection.
    pub async fn federation_chain_observation_state(
        &self,
    ) -> Result<FederationChainObservationState, Error> {
        match self
            .localstore()
            .kv_read(
                CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                CHAIN_OBSERVATION_STATE_KEY,
            )
            .await?
        {
            Some(bytes) => Ok(FederationChainObservationState::from_bytes(&bytes)?),
            None => Ok(FederationChainObservationState::new()),
        }
    }

    pub(super) async fn apply_federation_chain_observation(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationChainObservation,
    ) -> Result<FederationChainObservationOutcome, Error> {
        observation.validate()?;
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let public = runtime.public_config();
        let required_observers = public
            .payment_observation_policy
            .policy_for(FederationPaymentObservationContext::MintQuote)
            .required_matching_observations();
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let bytes = transaction
                .kv_read(
                    CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                    CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                    CHAIN_OBSERVATION_STATE_KEY,
                )
                .await?;
            let mut state = match bytes {
                Some(bytes) => FederationChainObservationState::from_bytes(&bytes)?,
                None => FederationChainObservationState::new(),
            };
            let outcome = match observation {
                FederationChainObservation::Deposit(observation) => {
                    let allocation_bytes = transaction
                        .kv_read(
                            ADDRESS_ALLOCATION_PRIMARY_NAMESPACE,
                            ADDRESS_ALLOCATION_SECONDARY_NAMESPACE,
                            ADDRESS_ALLOCATION_STATE_KEY,
                        )
                        .await?
                        .ok_or_else(|| {
                            cdk_common::federation::FederationError::ChainObservationInvalid {
                                reason: "deposit has no accepted allocation projection".to_string(),
                            }
                        })?;
                    let allocations =
                        FederationAddressAllocationState::from_bytes(&allocation_bytes)?;
                    let allocation = allocations
                        .allocation(
                            observation.allocation.scope(),
                            &observation.allocation.quote,
                        )
                        .ok_or_else(|| {
                            cdk_common::federation::FederationError::ChainObservationInvalid {
                                reason: "deposit references an unknown address allocation"
                                    .to_string(),
                            }
                        })?;
                    let wallet = public.frost_wallet.as_ref().ok_or_else(|| {
                        cdk_common::federation::FederationError::ChainObservationInvalid {
                            reason: "deposit requires an accepted FROST wallet config".to_string(),
                        }
                    })?;
                    let application = wallet
                        .cdk_onchain_applications
                        .iter()
                        .find(|application| {
                            application.application_id == observation.allocation.application_id
                                && application.application_key_epoch
                                    == observation.allocation.application_key_epoch
                        })
                        .ok_or_else(|| {
                            cdk_common::federation::FederationError::ChainObservationInvalid {
                                reason: "deposit references an unknown application epoch"
                                    .to_string(),
                            }
                        })?;
                    let expected_script = wallet.derive_cdk_onchain_script_for_known_epoch(
                        observation.allocation.application_id,
                        observation.allocation.application_key_epoch,
                        observation.allocation.keychain,
                        observation.allocation.derivation_index,
                    )?;
                    state.apply_deposit(
                        observation,
                        allocation,
                        &expected_script,
                        required_observers,
                        application.confirmation_policy.minimum_confirmations,
                        entry.operation_id,
                        entry.index,
                    )?
                }
                FederationChainObservation::Reorg(observation) => {
                    state.apply_reorg(observation, entry.operation_id, entry.index)?
                }
            };
            transaction
                .kv_write(
                    CHAIN_OBSERVATION_PRIMARY_NAMESPACE,
                    CHAIN_OBSERVATION_SECONDARY_NAMESPACE,
                    CHAIN_OBSERVATION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}
