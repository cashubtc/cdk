//! Thin materialized-state guard for reusable FROST root/application rotation.

use std::collections::BTreeMap;

use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAcceptedTransactionState, FederationFrostApplicationId,
    FederationFrostApplicationUsage, FederationFrostRotationGuard, FederationFrostRotationStatus,
    FederationFrostRotationTransition, FederationOnchainPaymentIntentState, FederationOperation,
    FederationOperationStatus, FederationTransactionPurpose, FederationWalletUtxoState,
};

use super::super::Mint;

impl Mint {
    /// Return accepted public root/application lifecycle metadata only.
    pub async fn federation_frost_rotation_status(
        &self,
    ) -> Result<Option<FederationFrostRotationStatus>, Error> {
        Ok(self
            .effective_federation_config()
            .await?
            .and_then(|runtime| {
                runtime
                    .public_config()
                    .frost_wallet
                    .as_ref()
                    .map(|wallet| wallet.rotation_status())
            }))
    }

    /// Fail closed before any caller deletes a participant-local root share.
    pub async fn ensure_federation_frost_root_share_deletable(
        &self,
        root_key_epoch: u64,
    ) -> Result<(), Error> {
        let runtime = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let wallet = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .ok_or(Error::Internal)?;
        wallet.ensure_root_share_deletable(
            root_key_epoch,
            &self.federation_frost_rotation_guard().await?,
        )?;
        Ok(())
    }

    /// Accepted journal index that activated one application epoch.
    pub async fn federation_frost_application_activation_index(
        &self,
        application_id: FederationFrostApplicationId,
        application_key_epoch: u64,
    ) -> Result<Option<u64>, Error> {
        Ok(self
            .localstore()
            .get_federation_journal_entries()
            .await?
            .into_iter()
            .find_map(|entry| {
                (entry.status() == FederationOperationStatus::Applied
                    && matches!(
                        &entry.envelope.operation,
                        FederationOperation::FrostRotation(operation)
                            if matches!(
                                operation.transition,
                                FederationFrostRotationTransition::ActivateApplication {
                                    application_id: found_id,
                                    application_key_epoch: found_epoch,
                                } if found_id == application_id
                                    && found_epoch == application_key_epoch
                            )
                    ))
                .then_some(entry.index)
            }))
    }

    /// Build retirement guards only from accepted wallet and transaction state.
    pub async fn federation_frost_rotation_guard(
        &self,
    ) -> Result<FederationFrostRotationGuard, Error> {
        let chain = self.federation_chain_observation_state().await?;
        let transactions = self.federation_transaction_state().await?;
        let mut usage =
            BTreeMap::<(FederationFrostApplicationId, u64), FederationFrostApplicationUsage>::new();

        for utxo in chain.utxos() {
            if matches!(
                utxo.state,
                FederationWalletUtxoState::Spent | FederationWalletUtxoState::Reorged
            ) {
                continue;
            }
            let value = usage
                .entry((
                    utxo.allocation.application_id,
                    utxo.allocation.application_key_epoch,
                ))
                .or_default();
            value.unspent_utxos = value.unspent_utxos.saturating_add(1);
            if utxo.state == FederationWalletUtxoState::Reserved {
                value.reserved_utxos = value.reserved_utxos.saturating_add(1);
            }
        }
        for intent in transactions.intents().values() {
            if matches!(
                intent.state,
                FederationOnchainPaymentIntentState::Pending
                    | FederationOnchainPaymentIntentState::Accepted { .. }
            ) {
                let value = usage
                    .entry((
                        intent.intent.scope.application_id,
                        intent.intent.scope.application_key_epoch,
                    ))
                    .or_default();
                value.unresolved_intents = value.unresolved_intents.saturating_add(1);
            }
        }
        for accepted in transactions.accepted_transactions().values() {
            if matches!(
                accepted.state,
                FederationAcceptedTransactionState::Accepted
                    | FederationAcceptedTransactionState::Signed
                    | FederationAcceptedTransactionState::BroadcastPending
                    | FederationAcceptedTransactionState::Broadcast
            ) {
                let value = usage
                    .entry((
                        accepted.proposal.scope.application_id,
                        accepted.proposal.scope.application_key_epoch,
                    ))
                    .or_default();
                value.unresolved_transactions = value.unresolved_transactions.saturating_add(1);
                if matches!(
                    &accepted.proposal.purpose,
                    FederationTransactionPurpose::PaymentBatch
                ) {
                    value.unresolved_payment_transactions =
                        value.unresolved_payment_transactions.saturating_add(1);
                }
            }
        }
        Ok(FederationFrostRotationGuard::new(usage))
    }
}
