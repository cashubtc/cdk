//! Thin persistence boundary for consensus-controlled address allocation.

use cdk_common::error::Error;
use cdk_common::federation::{
    read_federation_state, FederationAddressAllocationOperation,
    FederationAddressAllocationOutcome, FederationAddressAllocationScope,
    FederationAddressAllocationState, FederationJournalEntry, FederationStoredState,
};

use super::super::Mint;
use super::application::finish_federation_apply_transaction;

impl Mint {
    /// Load the complete validated address-allocation projection.
    pub async fn federation_address_allocation_state(
        &self,
    ) -> Result<FederationAddressAllocationState, Error> {
        read_federation_state(&self.localstore()).await
    }

    /// Look up one accepted quote mapping without touching a BDK cursor.
    pub async fn federation_address_allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: &str,
    ) -> Result<Option<cdk_common::federation::FederationAddressAllocation>, Error> {
        Ok(self
            .federation_address_allocation_state()
            .await?
            .allocation(scope, quote)
            .cloned())
    }

    pub(super) async fn apply_federation_address_allocation(
        &self,
        entry: &FederationJournalEntry,
        operation: &FederationAddressAllocationOperation,
    ) -> Result<FederationAddressAllocationOutcome, Error> {
        self.federation_config()
            .ok_or(Error::Internal)?
            .validate_federated_address_allocation_submission(operation)?;
        let mut transaction = self.localstore().begin_transaction().await?;
        let outcome = async {
            let mut state =
                FederationStoredState::<FederationAddressAllocationState>::load(&mut transaction)
                    .await?;
            let outcome = state.apply(operation, entry.operation_id, entry.index)?;
            state.save(&mut transaction).await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}
