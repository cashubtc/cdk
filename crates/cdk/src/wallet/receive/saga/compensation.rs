//! Compensation actions for the receive saga.
//!
//! When a saga step fails, compensating actions execute in reverse order (LIFO)
//! to undo completed steps and restore the database to its pre-saga state.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::wallet::ProofInfo;
use tracing::instrument;

use crate::nuts::PublicKey;
use crate::wallet::saga::CompensatingAction;
use crate::Error;

/// Compensation action to remove pending proofs that were stored during receive.
///
/// This compensation is used when receive fails after proofs have been stored
/// in Pending state (before the swap is executed). It removes those proofs,
/// puts back the rows the wallet stored for them before the receive, and
/// deletes the saga record.
pub struct RemovePendingProofs {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Y values (public keys) of the pending proofs to remove
    pub proof_ys: Vec<PublicKey>,
    /// Rows the wallet stored for these proofs before the receive. They are
    /// written back unchanged instead of being removed.
    pub restore_proofs: Vec<ProofInfo>,
    /// Saga ID, which is also the operation that owns the pending proofs
    pub saga_id: uuid::Uuid,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CompensatingAction for RemovePendingProofs {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Removing {} pending proofs from receive",
            self.proof_ys.len()
        );

        // The database only writes rows this receive still holds as pending.
        // A saved row whose proof was never overwritten, or has moved on since
        // (for example the send that owned it finished and removed it), is
        // left alone rather than brought back in a stale state.
        let restored_ys: HashSet<PublicKey> = self.restore_proofs.iter().map(|p| p.y).collect();
        let removed_ys = self
            .proof_ys
            .iter()
            .filter(|y| !restored_ys.contains(y))
            .copied()
            .collect();

        self.localstore
            .restore_pending_proofs(&self.saga_id, self.restore_proofs.clone(), removed_ys)
            .await
            .map_err(Error::Database)?;

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
                token: Some("test_token".to_string()),
                amount: Some(Amount::from(1000)),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
                replaced_proofs: Vec::new(),
            }),
        )
    }

    /// A proof the receive holds, as it writes it before the swap
    fn pending_proof_info(
        keyset_id: cdk_common::nuts::Id,
        amount: u64,
        mint_url: cdk_common::mint_url::MintUrl,
        saga_id: uuid::Uuid,
    ) -> ProofInfo {
        let mut proof_info = test_proof_info(keyset_id, amount, mint_url, State::Pending);
        proof_info.used_by_operation = Some(saga_id);
        proof_info
    }

    #[tokio::test]
    async fn test_remove_pending_proofs_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let saga = test_receive_saga(mint_url.clone());
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        let proof_info = pending_proof_info(keyset_id, 100, mint_url, saga_id);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            restore_proofs: Vec::new(),
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

        // Use a saga_id that doesn't exist
        let saga_id = uuid::Uuid::new_v4();

        let proof_info = pending_proof_info(keyset_id, 100, mint_url, saga_id);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            restore_proofs: Vec::new(),
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

        let saga = test_receive_saga(mint_url.clone());
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        // Create two pending proofs
        let proof_info_1 = pending_proof_info(keyset_id, 100, mint_url.clone(), saga_id);
        let proof_info_2 = pending_proof_info(keyset_id, 200, mint_url, saga_id);
        let proof_y_1 = proof_info_1.y;
        let proof_y_2 = proof_info_2.y;
        db.update_proofs(vec![proof_info_1, proof_info_2], vec![])
            .await
            .unwrap();

        // Only remove the first proof
        let compensation = RemovePendingProofs {
            localstore: db.clone(),
            proof_ys: vec![proof_y_1],
            restore_proofs: Vec::new(),
            saga_id,
        };
        compensation.execute().await.unwrap();

        // Second proof should still exist
        let remaining_proofs = db.get_proofs(None, None, None, None).await.unwrap();
        assert_eq!(remaining_proofs.len(), 1);
        assert_eq!(remaining_proofs[0].y, proof_y_2);
    }
}
