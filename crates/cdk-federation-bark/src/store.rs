//! Typed reads of the accepted Bark projection.
//!
//! The mint database writes the projection while it applies accepted
//! operations. Validation on the apply path reads it through these keyed
//! queries, so no apply-path read grows with retained history. The whole
//! snapshot remains for the Bark drivers, operator status and checkpoints.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::Error;

use crate::{
    FederationBarkActionId, FederationBarkActionRecord, FederationBarkObservationRecord,
    FederationBarkProjectionSnapshot, FederationBarkTransitionRecord, FederationBarkVtxoId,
    FederationBarkVtxoRecord, FederationBarkVtxoState,
};

/// Keyed, typed reads of the accepted Bark projection.
#[async_trait]
pub trait FederationBarkStore: fmt::Debug + Send + Sync {
    /// One action head.
    async fn bark_action(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Option<FederationBarkActionRecord>, Error>;

    /// One action's accepted transitions, in consensus order.
    async fn bark_action_transitions(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkTransitionRecord>, Error>;

    /// One action's accepted member observations, in consensus order.
    async fn bark_action_observations(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkObservationRecord>, Error>;

    /// VTXOs currently in one state, ordered by VTXO id.
    async fn bark_vtxos_in_state(
        &self,
        state: FederationBarkVtxoState,
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error>;

    /// The VTXOs with these ids that exist, ordered by VTXO id.
    async fn bark_vtxos(
        &self,
        vtxo_ids: &[FederationBarkVtxoId],
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error>;

    /// VTXOs reserved by one action, ordered by VTXO id.
    async fn bark_vtxos_reserved_by(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error>;

    /// Highest allocated custody locator index, if any locator was allocated.
    async fn bark_max_locator_index(&self) -> Result<Option<u32>, Error>;

    /// Highest accepted operation index of any stored Bark row, which is the
    /// [`FederationBarkProjectionSnapshot::frontier`] of the whole projection,
    /// read without loading it.
    async fn bark_projection_frontier(&self) -> Result<Option<u64>, Error>;

    /// The whole projection, for the drivers, operator status and checkpoints.
    async fn bark_projection_snapshot(&self) -> Result<FederationBarkProjectionSnapshot, Error>;
}

/// Shared handle to a Bark projection store.
pub type DynFederationBarkStore = Arc<dyn FederationBarkStore>;
