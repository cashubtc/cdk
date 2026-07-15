//! Thin persistence boundary for consensus-controlled address allocation.

use cdk_common::error::Error;
use cdk_common::federation::{
    FederationAddressAllocationOperation, FederationAddressAllocationOutcome,
    FederationAddressAllocationScope, FederationAddressAllocationState, FederationJournalEntry,
};

use super::super::Mint;
use super::finish_federation_apply_transaction;

pub(super) const ADDRESS_ALLOCATION_PRIMARY_NAMESPACE: &str = "federation";
pub(super) const ADDRESS_ALLOCATION_SECONDARY_NAMESPACE: &str = "address_allocation";
pub(super) const ADDRESS_ALLOCATION_STATE_KEY: &str = "state_v1";

impl Mint {
    /// Load the complete validated address-allocation projection.
    pub async fn federation_address_allocation_state(
        &self,
    ) -> Result<FederationAddressAllocationState, Error> {
        match self
            .localstore()
            .kv_read(
                ADDRESS_ALLOCATION_PRIMARY_NAMESPACE,
                ADDRESS_ALLOCATION_SECONDARY_NAMESPACE,
                ADDRESS_ALLOCATION_STATE_KEY,
            )
            .await?
        {
            Some(bytes) => Ok(FederationAddressAllocationState::from_bytes(&bytes)?),
            None => Ok(FederationAddressAllocationState::new()),
        }
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
            let bytes = transaction
                .kv_read(
                    ADDRESS_ALLOCATION_PRIMARY_NAMESPACE,
                    ADDRESS_ALLOCATION_SECONDARY_NAMESPACE,
                    ADDRESS_ALLOCATION_STATE_KEY,
                )
                .await?;
            let mut state = match bytes {
                Some(bytes) => FederationAddressAllocationState::from_bytes(&bytes)?,
                None => FederationAddressAllocationState::new(),
            };
            let outcome = state.apply(operation, entry.operation_id, entry.index)?;
            transaction
                .kv_write(
                    ADDRESS_ALLOCATION_PRIMARY_NAMESPACE,
                    ADDRESS_ALLOCATION_SECONDARY_NAMESPACE,
                    ADDRESS_ALLOCATION_STATE_KEY,
                    &state.to_bytes()?,
                )
                .await?;
            Ok(outcome)
        }
        .await;
        finish_federation_apply_transaction(transaction, entry, outcome).await
    }
}
