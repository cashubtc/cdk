//! Compensation actions for the receive saga.
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.

use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, WalletDatabase};
use tracing::instrument;

use crate::nuts::PublicKey;
use crate::wallet::saga::CompensatingAction;
use crate::Error;

/// Compensation action to remove pending proofs that were stored during receive.
///
/// This compensation is used when receive fails after proofs have been stored
/// in Pending state (before the swap is executed). It removes those proofs
/// and deletes the saga record.
pub struct RemovePendingProofs {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Y values (public keys) of the pending proofs to remove
    pub proof_ys: Vec<PublicKey>,
    /// Saga ID for cleanup
    pub saga_id: uuid::Uuid,
}

#[async_trait]
impl CompensatingAction for RemovePendingProofs {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Removing {} pending proofs from receive",
            self.proof_ys.len()
        );

        self.localstore
            .update_proofs(vec![], self.proof_ys.clone())
            .await
            .map_err(Error::Database)?;

        // Delete saga record (best-effort)
        if let Err(e) = self.localstore.delete_saga(&self.saga_id).await {
            tracing::warn!(
                "Compensation: Failed to delete saga {}: {}. Will be cleaned up on recovery.",
                self.saga_id,
                e
            );
            // Don't fail compensation if saga deletion fails - orphaned saga is harmless
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "RemovePendingProofs"
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::wallet::{
        OperationData, ReceiveOperationData, ReceiveSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use super::*;
    use crate::wallet::saga::test_utils::*;
    use crate::wallet::saga::CompensatingAction;

    /// Create a test wallet saga for receive operations
    fn test_receive_saga(mint_url: cdk_common::mint_url::MintUrl) -> WalletSaga {
        WalletSaga::new(
            uuid::Uuid::new_v4(),
            WalletSagaState::Receive(ReceiveSagaState::ProofsPending),
            Amount::from(1000),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Receive(ReceiveOperationData {
                token: "test_token".to_string(),
                amount: Some(Amount::from(1000)),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        )
    }

    #[tokio::test]
    async fn test_remove_pending_proofs_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Pending);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = test_receive_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Execute twice - should succeed both times
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        // Proofs should still be gone
        let all_proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(all_proofs.is_empty());
    }

    #[tokio::test]
    async fn test_remove_pending_proofs_handles_missing_saga() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Pending);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        // Use a saga_id that doesn't exist
        let saga_id = uuid::Uuid::new_v4();

        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Should succeed even without saga
        compensation.execute().await.unwrap();

        // Proofs should be removed
        let all_proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert!(all_proofs.is_empty());
    }

    #[tokio::test]
    async fn test_remove_pending_proofs_only_affects_specified_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create two pending proofs
        let proof_info_1 = test_proof_info(keyset_id, 100, mint_url.clone(), State::Pending);
        let proof_info_2 = test_proof_info(keyset_id, 200, mint_url.clone(), State::Pending);
        let proof_y_1 = proof_info_1.y;
        let proof_y_2 = proof_info_2.y;
        db.update_proofs(vec![proof_info_1, proof_info_2], vec![])
            .await
            .unwrap();

        let saga = test_receive_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        // Only remove the first proof
        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y_1],
            saga_id,
        };
        compensation.execute().await.unwrap();

        // Second proof should still exist
        let remaining_proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert_eq!(remaining_proofs.len(), 1);
        assert_eq!(remaining_proofs[0].y, proof_y_2);
    }
}
