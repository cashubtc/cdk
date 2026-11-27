//! Thin persistence boundary for consensus-controlled address allocation.

use cdk_common::error::Error;
use cdk_common::federation::{
    apply_federation_address_allocation_in_tx, read_federation_address_allocation,
    read_federation_state, FederationAddressAllocationOperation,
    FederationAddressAllocationOutcome, FederationAddressAllocationScope,
    FederationAddressAllocationState, FederationJournalEntry,
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
        read_federation_address_allocation(&self.localstore(), scope, quote).await
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
        let outcome = apply_federation_address_allocation_in_tx(
            &mut transaction,
            operation,
            entry.operation_id,
            entry.index,
        )
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}
