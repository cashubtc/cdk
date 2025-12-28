//! Wallet Saga Pattern Implementation
//!
//! This module provides the type state pattern infrastructure for wallet operations.
//! It mirrors the mint's saga pattern to ensure consistency across the codebase.
//!
//! # Type State Pattern
//!
//! The type state pattern uses Rust's type system to enforce valid state transitions
//! at compile-time. Each operation state is a distinct type, and operations are only
//! available on the appropriate type.
//!
//! # Compensation Pattern
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::{self, WalletDatabase};
use tokio::sync::Mutex;
use tracing::instrument;

use crate::nuts::{PublicKey, State};
use crate::Error;

/// Trait for compensating actions in the saga pattern.
///
/// Compensating actions are registered as steps complete and executed in reverse
/// order (LIFO) if the saga fails. Each action should be idempotent.
#[async_trait]
pub trait CompensatingAction: Send + Sync {
    /// Execute the compensating action
    async fn execute(&self) -> Result<(), Error>;

    /// Get the name of this compensating action for logging
    fn name(&self) -> &'static str;
}

/// A queue of compensating actions for saga rollback.
///
/// Actions are stored in LIFO order (most recent first) and executed
/// in that order during compensation.
pub type Compensations = Arc<Mutex<VecDeque<Box<dyn CompensatingAction>>>>;

/// Create a new empty compensations queue
pub fn new_compensations() -> Compensations {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// Execute all compensating actions in the queue.
///
/// Actions are executed in LIFO order (most recent first).
/// Errors during compensation are logged but don't stop the process.
pub async fn execute_compensations(compensations: &Compensations) -> Result<(), Error> {
    let mut queue = compensations.lock().await;

    if queue.is_empty() {
        return Ok(());
    }

    tracing::warn!("Running {} compensating actions", queue.len());

    while let Some(compensation) = queue.pop_front() {
        tracing::debug!("Running compensation: {}", compensation.name());
        if let Err(e) = compensation.execute().await {
            tracing::error!(
                "Compensation {} failed: {}. Continuing...",
                compensation.name(),
                e
            );
        }
    }

    Ok(())
}

/// Clear all compensating actions from the queue.
///
/// Called when an operation completes successfully.
pub async fn clear_compensations(compensations: &Compensations) {
    compensations.lock().await.clear();
}

/// Add a compensating action to the front of the queue (LIFO order).
pub async fn add_compensation(compensations: &Compensations, action: Box<dyn CompensatingAction>) {
    compensations.lock().await.push_front(action);
}

// ============================================================================
// Shared Compensation Actions
// ============================================================================

/// Compensation action to revert proof reservation.
///
/// This compensation is used when a saga fails after proofs have been reserved.
/// It sets the proofs back to Unspent state and deletes the saga record.
///
/// Used by: send, melt, and swap sagas.
pub struct RevertProofReservation {
    /// Database reference
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Y values (public keys) of the reserved proofs
    pub proof_ys: Vec<PublicKey>,
    /// Saga ID for cleanup
    pub saga_id: uuid::Uuid,
}

#[async_trait]
impl CompensatingAction for RevertProofReservation {
    #[instrument(skip_all)]
    async fn execute(&self) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Reverting {} proofs from Reserved to Unspent",
            self.proof_ys.len()
        );

        self.localstore
            .update_proofs_state(self.proof_ys.clone(), State::Unspent)
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
        "RevertProofReservation"
    }
}

/// Test utilities shared across wallet saga compensation tests.
#[cfg(test)]
pub mod test_utils {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;
    use cdk_common::nuts::{CurrencyUnit, Id, Proof, State};
    use cdk_common::secret::Secret;
    use cdk_common::{Amount, SecretKey};

    use crate::types::ProofInfo;

    /// Create an in-memory test database
    pub async fn create_test_db(
    ) -> Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> {
        Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap())
    }

    /// Create a test keyset ID
    pub fn test_keyset_id() -> Id {
        Id::from_str("00916bbf7ef91a36").unwrap()
    }

    /// Create a test mint URL
    pub fn test_mint_url() -> cdk_common::mint_url::MintUrl {
        cdk_common::mint_url::MintUrl::from_str("https://test-mint.example.com").unwrap()
    }

    /// Create a test proof with the given keyset ID and amount
    pub fn test_proof(keyset_id: Id, amount: u64) -> Proof {
        Proof {
            amount: Amount::from(amount),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        }
    }

    /// Create a test proof info with the given parameters
    pub fn test_proof_info(
        keyset_id: Id,
        amount: u64,
        mint_url: cdk_common::mint_url::MintUrl,
        state: State,
    ) -> ProofInfo {
        let proof = test_proof(keyset_id, amount);
        ProofInfo::new(proof, mint_url, state, CurrencyUnit::Sat).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock compensating action for testing that tracks execution order.
    struct MockCompensation {
        name: &'static str,
        execution_order: Arc<std::sync::Mutex<Vec<&'static str>>>,
        should_fail: bool,
    }

    impl MockCompensation {
        fn new(
            name: &'static str,
            execution_order: Arc<std::sync::Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                name,
                execution_order,
                should_fail: false,
            }
        }

        fn failing(
            name: &'static str,
            execution_order: Arc<std::sync::Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                name,
                execution_order,
                should_fail: true,
            }
        }
    }

    #[async_trait]
    impl CompensatingAction for MockCompensation {
        async fn execute(&self) -> Result<(), Error> {
            self.execution_order.lock().unwrap().push(self.name);
            if self.should_fail {
                Err(Error::Custom("Intentional test failure".to_string()))
            } else {
                Ok(())
            }
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn test_compensations_lifo_order() {
        // Test that compensations execute in LIFO (most-recent-first) order
        let compensations = new_compensations();
        let execution_order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Add compensations in order: first, second, third
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("first", execution_order.clone())),
        )
        .await;
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("second", execution_order.clone())),
        )
        .await;
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("third", execution_order.clone())),
        )
        .await;

        // Execute compensations
        execute_compensations(&compensations).await.unwrap();

        // Verify LIFO order: third (most recent) should execute first
        let order = execution_order.lock().unwrap();
        assert_eq!(order.as_slice(), &["third", "second", "first"]);
    }

    #[tokio::test]
    async fn test_compensations_continues_on_error() {
        // Test that one failed compensation doesn't stop others from executing
        let compensations = new_compensations();
        let execution_order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Add: first (will succeed), second (will fail), third (will succeed)
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("first", execution_order.clone())),
        )
        .await;
        add_compensation(
            &compensations,
            Box::new(MockCompensation::failing(
                "second_fails",
                execution_order.clone(),
            )),
        )
        .await;
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("third", execution_order.clone())),
        )
        .await;

        // Execute compensations - should complete without error even though one failed
        let result = execute_compensations(&compensations).await;
        assert!(result.is_ok());

        // All three should have executed despite the middle one failing
        let order = execution_order.lock().unwrap();
        assert_eq!(order.as_slice(), &["third", "second_fails", "first"]);
    }

    #[tokio::test]
    async fn test_compensations_empty_queue() {
        // Test that empty queue operations work correctly
        let compensations = new_compensations();

        // Execute on empty queue should succeed
        let result = execute_compensations(&compensations).await;
        assert!(result.is_ok());

        // Clear on empty queue should succeed
        clear_compensations(&compensations).await;

        // Queue should still be empty
        assert!(compensations.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_clear_compensations() {
        // Test that clear_compensations removes all actions
        let compensations = new_compensations();
        let execution_order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Add some compensations
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("first", execution_order.clone())),
        )
        .await;
        add_compensation(
            &compensations,
            Box::new(MockCompensation::new("second", execution_order.clone())),
        )
        .await;

        // Verify queue is not empty
        assert!(!compensations.lock().await.is_empty());

        // Clear the queue
        clear_compensations(&compensations).await;

        // Verify queue is now empty
        assert!(compensations.lock().await.is_empty());

        // Execute should do nothing
        execute_compensations(&compensations).await.unwrap();
        assert!(execution_order.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_new_compensations_creates_empty_queue() {
        // Test that new_compensations creates an empty queue
        let compensations = new_compensations();
        assert!(compensations.lock().await.is_empty());
    }
}
