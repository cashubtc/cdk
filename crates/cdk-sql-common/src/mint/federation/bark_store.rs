//! Keyed reads of the accepted Bark projection.

use cdk_federation_bark::{FederationBarkActionId, FederationBarkStore, FederationBarkVtxoId};

use super::*;

#[async_trait]
impl<RM> FederationBarkStore for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn bark_action(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Option<FederationBarkActionRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_action(&*conn, action_id).await
    }

    async fn bark_action_transitions(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkTransitionRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_action_transitions(&*conn, action_id).await
    }

    async fn bark_action_observations(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkObservationRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_action_observations(&*conn, action_id).await
    }

    async fn bark_vtxos_in_state(
        &self,
        state: FederationBarkVtxoState,
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_vtxos_in_state(&*conn, state).await
    }

    async fn bark_vtxos(
        &self,
        vtxo_ids: &[FederationBarkVtxoId],
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_vtxos(&*conn, vtxo_ids).await
    }

    async fn bark_vtxos_reserved_by(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Vec<FederationBarkVtxoRecord>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_reserved_vtxos(&*conn, action_id).await
    }

    async fn bark_max_locator_index(&self) -> Result<Option<u32>, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_max_locator_index(&*conn).await
    }

    async fn bark_projection_snapshot(&self) -> Result<FederationBarkProjectionSnapshot, Error> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        bark_projection::get_bark_projection_snapshot(&*conn).await
    }
}
