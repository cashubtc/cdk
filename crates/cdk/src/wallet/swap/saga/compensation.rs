//! Compensation actions for the swap saga.
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.

// Re-export shared compensation action used by swap saga
// Note: RevertSwapProofReservation was renamed to the shared RevertProofReservation
pub use crate::wallet::saga::RevertProofReservation as RevertSwapProofReservation;

#[cfg(test)]
mod tests {
    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::wallet::{
        OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::Amount;

    use super::*;
    use crate::wallet::saga::test_utils::*;
    use crate::wallet::saga::CompensatingAction;

    /// Create a test wallet saga for swap operations
    fn test_swap_saga(mint_url: cdk_common::mint_url::MintUrl) -> WalletSaga {
        WalletSaga::new(
            uuid::Uuid::new_v4(),
            WalletSagaState::Swap(SwapSagaState::ProofsReserved),
            Amount::from(1000),
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Swap(SwapOperationData {
                input_amount: Amount::from(1000),
                output_amount: Amount::from(990),
                counter_start: Some(0),
                counter_end: Some(10),
                blinded_messages: None,
            }),
        )
    }

    #[tokio::test]
    async fn test_revert_swap_proof_reservation_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = test_swap_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        let compensation = RevertSwapProofReservation {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Execute twice - should succeed both times
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        // Proof should be Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
    }

    #[tokio::test]
    async fn test_revert_swap_proof_reservation_handles_missing_saga() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        // Use a saga_id that doesn't exist
        let saga_id = uuid::Uuid::new_v4();

        let compensation = RevertSwapProofReservation {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Should succeed even without saga
        compensation.execute().await.unwrap();

        // Proof should be Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
    }

    #[tokio::test]
    async fn test_revert_swap_proof_reservation_only_affects_specified_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create two reserved proofs
        let proof_info_1 = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_info_2 = test_proof_info(keyset_id, 200, mint_url.clone(), State::Reserved);
        let proof_y_1 = proof_info_1.y;
        let proof_y_2 = proof_info_2.y;
        db.update_proofs(vec![proof_info_1, proof_info_2], vec![])
            .await
            .unwrap();

        let saga = test_swap_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        // Only revert the first proof
        let compensation = RevertSwapProofReservation {
            localstore: db.clone(),
            proof_ys: vec![proof_y_1],
            saga_id,
        };
        compensation.execute().await.unwrap();

        // First proof should be Unspent
        let unspent = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(unspent.len(), 1);
        assert_eq!(unspent[0].y, proof_y_1);

        // Second proof should still be Reserved
        let reserved = db
            .get_proofs(None, None, Some(vec![State::Reserved]), None)
            .await
            .unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].y, proof_y_2);
    }
}
