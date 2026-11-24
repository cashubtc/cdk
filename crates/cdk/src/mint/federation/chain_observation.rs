//! Thin persistence boundary for consensus-controlled Bitcoin chain evidence.

use cdk_common::error::Error;
use cdk_common::federation::{
    read_federation_state, FederationAddressAllocationState, FederationChainObservation,
    FederationChainObservationOutcome, FederationChainObservationState, FederationJournalEntry,
    FederationPaymentObservationContext, FederationStoredState,
};

use super::super::Mint;
use super::application::finish_federation_apply_transaction;

impl Mint {
    /// Load the complete validated consensus wallet UTXO projection.
    pub async fn federation_chain_observation_state(
        &self,
    ) -> Result<FederationChainObservationState, Error> {
        read_federation_state(&self.localstore()).await
    }

    pub(super) async fn apply_federation_chain_observation(
        &self,
        entry: &FederationJournalEntry,
        observation: &FederationChainObservation,
    ) -> Result<FederationChainObservationOutcome, Error> {
        observation.validate()?;
        let runtime = self.federation_config().ok_or(Error::Internal)?;
        let public = runtime.public_config();
        let required_observers = public
            .payment_observation_policy
            .policy_for(FederationPaymentObservationContext::MintQuote)
            .required_matching_observations();
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state =
                FederationStoredState::<FederationChainObservationState>::load(&mut transaction)
                    .await?;
            let outcome = match observation {
                FederationChainObservation::Deposit(observation) => {
                    let allocations =
                        FederationStoredState::<FederationAddressAllocationState>::load(
                            &mut transaction,
                        )
                        .await?
                        .into_inner();
                    let allocation = allocations
                        .allocation(
                            observation.allocation.scope(),
                            &observation.allocation.quote,
                        )
                        .ok_or({
                            cdk_common::federation::FederationError::ChainObservationInvalid {
                                reason: "deposit references an unknown address allocation",
                            }
                        })?;
                    let wallet = public.frost_wallet.as_ref().ok_or({
                        cdk_common::federation::FederationError::ChainObservationInvalid {
                            reason: "deposit requires an accepted FROST wallet config",
                        }
                    })?;
                    let cdk_application = wallet.cdk_onchain_application.as_ref().filter(
                        |application| {
                            application.application_id
                                == observation.allocation.application_id
                                && application.application_key_epoch
                                    == observation.allocation.application_key_epoch
                        },
                    );
                    let expected = cdk_application
                        .map(|application| {
                            wallet
                                .derive_cdk_onchain_script(
                                    public,
                                    observation.allocation.application_id,
                                    observation.allocation.application_key_epoch,
                                    observation.allocation.keychain,
                                    observation.allocation.derivation_index,
                                )
                                .map(|script| {
                                    (
                                        script,
                                        application.confirmation_policy.minimum_confirmations,
                                    )
                                })
                        })
                        .transpose()?;
                    #[cfg(feature = "federation-bark")]
                    let expected = match expected {
                        Some(expected) => Some(expected),
                        None => cdk_federation_bark::federation_bark_recovery_application(public)?
                            .filter(|application| {
                                application.application_id()
                                    == observation.allocation.application_id
                                    && application.application_key_epoch()
                                        == observation.allocation.application_key_epoch
                            })
                            .map(|application| {
                                application
                                    .derive_script(
                                        public,
                                        &wallet.root,
                                        observation.allocation.keychain
                                            == cdk_common::federation::FederationAddressKeychain::Internal,
                                        observation.allocation.derivation_index,
                                    )
                                    .map(|script| {
                                        (script, application.minimum_confirmations())
                                    })
                            })
                            .transpose()?,
                    };
                    let (expected_script, minimum_confirmations) = expected.ok_or({
                        cdk_common::federation::FederationError::ChainObservationInvalid {
                            reason: "deposit references an unknown descriptor application epoch",
                        }
                    })?;
                    state.apply_deposit(
                        observation,
                        allocation,
                        &expected_script,
                        required_observers,
                        minimum_confirmations,
                        entry.operation_id,
                        entry.index,
                    )?
                }
                FederationChainObservation::Reorg(observation) => {
                    state.apply_reorg(observation, entry.operation_id, entry.index)?
                }
            };
            state.save(&mut transaction).await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}
