//! Compensation actions for the mint (issue) saga.
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.
//!
//! Note: For mint operations, the primary side effect before the API call is
//! incrementing the keyset counter. Counter increments are not reversed because:
//! 1. They don't cause data loss (just potentially unused counter values)
//! 2. The secrets can be recovered via the restore process
//! 3. Reversing could cause issues if concurrent operations used adjacent counters

use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, WalletDatabase};
use tracing::instrument;
use uuid::Uuid;

use crate::wallet::saga::CompensatingAction;
use crate::Error;

/// Compensation action to release a mint quote reservation.
///
/// This compensation is used when mint fails after the quote has been reserved
/// but before it has been used. It clears the used_by_operation field on the quote.
pub struct ReleaseMintQuote {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Operation ID that reserved the quote
    pub operation_id: Uuid,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CompensatingAction for ReleaseMintQuote {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Releasing mint quote reserved by operation {}",
            self.operation_id
        );

        self.localstore
            .release_mint_quote(&self.operation_id)
            .await
            .map_err(Error::Database)?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "ReleaseMintQuote"
    }
}

/// Placeholder compensation action for mint operations.
///
/// Currently, mint operations don't require compensation because:
/// - Counter increments are intentionally not reversed
/// - No proofs are stored until after successful mint
/// - Quote state is not modified until after successful mint
///
/// This struct exists for consistency with other sagas and for
/// potential future use if mint recovery logic changes.
pub struct MintCompensation {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Quote ID (for logging)
    pub quote_id: String,
    /// Saga ID for cleanup
    pub saga_id: uuid::Uuid,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CompensatingAction for MintCompensation {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Mint operation for quote {} failed, no rollback needed",
            self.quote_id
        );

        if let Err(e) = self.localstore.delete_saga(&self.saga_id).await {
            tracing::warn!(
                "Compensation: Failed to delete saga {}: {}. Will be cleaned up on recovery.",
                self.saga_id,
                e
            );
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "MintCompensation"
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::CurrencyUnit;
    use cdk_common::wallet::{
        MintQuote, OperationData, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::{Amount, PaymentMethod};

    use super::*;
    use crate::wallet::saga::test_utils::*;
    use crate::wallet::saga::CompensatingAction;

    /// Create a test wallet saga for issue operations
    fn test_issue_saga(mint_url: cdk_common::mint_url::MintUrl) -> WalletSaga {
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

    /// Create a test mint quote
    fn test_mint_quote(mint_url: cdk_common::mint_url::MintUrl) -> MintQuote {
        MintQuote::new(
            format!("test_quote_{}", uuid::Uuid::new_v4()),
            mint_url,
            PaymentMethod::Known(KnownMethod::Bolt11),
            Some(Amount::from(1000)),
            CurrencyUnit::Sat,
            "lnbc1000...".to_string(),
            9999999999,
            None,
        )
    }

    // =========================================================================
    // ReleaseMintQuote Tests
    // =========================================================================

    #[tokio::test]
    async fn test_release_mint_quote_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let operation_id = uuid::Uuid::new_v4();

        let mut quote = test_mint_quote(mint_url);
        quote.used_by_operation = Some(operation_id.to_string());
        db.add_mint_quote(quote.clone()).await.unwrap();

        let compensation = ReleaseMintQuote {
            localstore: db.clone(),
            operation_id,
        };

        // Execute twice
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        let retrieved_quote = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
        assert!(retrieved_quote.used_by_operation.is_none());
    }

    #[tokio::test]
    async fn test_release_mint_quote_handles_no_matching_quote() {
        let db = create_test_db().await;
        let operation_id = uuid::Uuid::new_v4();

        // Don't add any quote - compensation should still succeed
        let compensation = ReleaseMintQuote {
            localstore: db.clone(),
            operation_id,
        };

        // Should not error even with no matching quote
        let result = compensation.execute().await;
        assert!(result.is_ok());
    }

    // =========================================================================
    // MintCompensation Tests
    // =========================================================================

    #[tokio::test]
    async fn test_mint_compensation_is_idempotent() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();

        let saga = test_issue_saga(mint_url);
        let saga_id = saga.id;
        db.add_saga(saga).await.unwrap();

        let compensation = MintCompensation {
            localstore: db.clone(),
            quote_id: "test_quote".to_string(),
            saga_id,
        };

        // Execute twice - should succeed both times
        compensation.execute().await.unwrap();
        compensation.execute().await.unwrap();

        assert!(db.get_saga(&saga_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_mint_compensation_handles_missing_saga() {
        let db = create_test_db().await;
        let saga_id = uuid::Uuid::new_v4();

        let compensation = MintCompensation {
            localstore: db.clone(),
            quote_id: "test_quote".to_string(),
            saga_id,
        };

        // Should succeed even without saga
        let result = compensation.execute().await;
        assert!(result.is_ok());
    }
}
