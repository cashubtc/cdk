//! Compensation actions for the melt saga.
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.

use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, WalletDatabase};
use tracing::instrument;
use uuid::Uuid;

use crate::wallet::saga::CompensatingAction;
// Re-export shared compensation actions used by melt saga
pub(crate) use crate::wallet::saga::RevertProofReservation;
use crate::Error;

/// Compensation action to release a melt quote reservation.
///
/// This compensation is used when melt fails after the quote has been reserved
/// but before it has been used. It clears the used_by_operation field on the quote.
pub struct ReleaseMeltQuote {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Operation ID that reserved the quote
    pub operation_id: Uuid,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CompensatingAction for ReleaseMeltQuote {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Releasing melt quote reserved by operation {}",
            self.operation_id
        );

        self.localstore
            .release_melt_quote(&self.operation_id)
            .await
            .map_err(Error::Database)?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "ReleaseMeltQuote"
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{CurrencyUnit, MeltQuoteState, State};
    use cdk_common::wallet::{
        MeltQuote, OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::{Amount, PaymentMethod};

    use super::*;
    use crate::wallet::saga::test_utils::*;
    use crate::wallet::saga::CompensatingAction;

    /// Create a test wallet saga for melt operations
    fn test_melt_saga(mint_url: cdk_common::mint_url::MintUrl) -> WalletSaga {
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

    /// Create a test melt quote
    fn test_melt_quote() -> MeltQuote {
        MeltQuote {
            id: format!("test_melt_quote_{}", uuid::Uuid::new_v4()),
            unit: CurrencyUnit::Sat,
            amount: Amount::from(1000),
            request: "lnbc1000...".to_string(),
            fee_reserve: Amount::from(10),
            state: MeltQuoteState::Unpaid,
            expiry: 9999999999,
            payment_preimage: None,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            used_by_operation: None,
            version: 0,
        }
    }

    // =========================================================================
    // RevertProofReservation Tests
    // =========================================================================

    #[tokio::test]
    async fn test_revert_proof_reservation_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create and store proof in Reserved state
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let saga = test_melt_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        let compensation = RevertProofReservation {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Execute twice - should succeed both times
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        // Proof should still be Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
    }

    #[tokio::test]
    async fn test_revert_proof_reservation_handles_missing_saga() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create and store proof
        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        // Use a saga_id that doesn't exist
        let saga_id = uuid::Uuid::new_v4();

        let compensation = RevertProofReservation {
            localstore: db.clone(),
            proof_ys: vec![proof_y],
            saga_id,
        };

        // Should succeed even though saga doesn't exist
        compensation.execute().await.unwrap();

        // Proof should be Unspent
        let proofs = db
            .get_proofs(None, None, Some(vec![State::Unspent]), None)
            .await
            .unwrap();
        assert_eq!(proofs.len(), 1);
    }

    // =========================================================================
    // ReleaseMeltQuote Tests
    // =========================================================================

    #[tokio::test]
    async fn test_release_melt_quote_is_idempotent() {
        let db = create_test_db().await;
        let operation_id = uuid::Uuid::new_v4();

        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(operation_id.to_string());
        db.add_melt_quote(quote.clone()).await.unwrap();

        let compensation = ReleaseMeltQuote {
            localstore: db.clone(),
            operation_id,
        };

        // Execute twice
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        let retrieved_quote = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());
    }

    #[tokio::test]
    async fn test_release_melt_quote_handles_no_matching_quote() {
        let db = create_test_db().await;
        let operation_id = uuid::Uuid::new_v4();

        // Don't add any quote - compensation should still succeed
        let compensation = ReleaseMeltQuote {
            localstore: db.clone(),
            operation_id,
        };

        // Should not error even with no matching quote
        let result = compensation.execute().await;
        assert!(result.is_ok());
    }

    // =========================================================================
    // Isolation Tests
    // =========================================================================

    #[tokio::test]
    async fn test_compensation_only_affects_specified_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();

        // Create two proofs, both Reserved
        let proof_info_1 = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_info_2 = test_proof_info(keyset_id, 200, mint_url.clone(), State::Reserved);

        let proof_y_1 = proof_info_1.y;
        let proof_y_2 = proof_info_2.y;

        db.update_proofs(vec![proof_info_1, proof_info_2], vec![])
            .await
            .unwrap();

        let saga = test_melt_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        // Only revert the first proof
        let compensation = RevertProofReservation {
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
