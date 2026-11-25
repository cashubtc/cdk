//! Deletion of terminal federation entities below the retention floor.
//!
//! Operations never read a terminal entity again: projections retire them
//! when they load, payment observations of terminal quotes are left out of
//! every read. What is left is storage. Below the retention floor every later
//! checkpoint's materialized-state digest already leaves them out, so a
//! member deletes them at its own pace and still agrees with every peer.

use cdk_common::error::Error;
use cdk_common::federation::{
    retire_federation_state_below, FederationChainObservationState, FederationStateTable,
    FederationTransactionState,
};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::FederationBarkRecoveryState;
use tracing::instrument;

use crate::mint::Mint;

impl Mint {
    /// Delete one bounded batch of terminal entities retired below `floor`,
    /// each store in its own transaction, and return how many rows changed.
    ///
    /// Spent wallet outputs leave a permanent outpoint tombstone. Signature
    /// shares, address allocations, proofs and blind signatures are never
    /// deleted.
    #[instrument(skip(self))]
    pub async fn prune_federation_terminal_state(
        &self,
        floor: u64,
        limit: u64,
    ) -> Result<u64, Error> {
        let entities = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut changed = 0_u64;
        changed = changed.saturating_add(
            self.retire_projection_below::<FederationChainObservationState>(floor, entities)
                .await?,
        );
        changed = changed.saturating_add(
            self.retire_projection_below::<FederationTransactionState>(floor, entities)
                .await?,
        );
        #[cfg(feature = "federation-bark")]
        {
            changed = changed.saturating_add(
                self.retire_projection_below::<FederationBarkRecoveryState>(floor, entities)
                    .await?,
            );
        }
        let mut tx = self.localstore().begin_transaction().await?;
        let rows = tx.prune_federation_terminal_rows(floor, limit).await?;
        tx.commit().await?;
        Ok(changed.saturating_add(rows))
    }

    async fn retire_projection_below<S>(&self, floor: u64, limit: usize) -> Result<u64, Error>
    where
        S: FederationStateTable,
    {
        let mut tx = self.localstore().begin_transaction().await?;
        let changed = retire_federation_state_below::<S>(&mut tx, floor, limit).await?;
        tx.commit().await?;
        Ok(changed)
    }
}
