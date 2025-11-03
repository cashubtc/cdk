//! Compensation actions for the melt saga pattern.
//!
//! When a saga step fails, compensating actions are executed in reverse order (LIFO)
//! to undo all completed steps and restore the database to its pre-saga state.

use async_trait::async_trait;
use cdk_common::database::DynMintDatabase;
use cdk_common::{Error, PublicKey, QuoteId};
use tracing::instrument;

/// Trait for compensating actions in the saga pattern.
///
/// Compensating actions are registered as steps complete and executed in reverse
/// order (LIFO) if the saga fails. Each action should be idempotent.
#[async_trait]
pub trait CompensatingAction: Send + Sync {
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error>;
    fn name(&self) -> &'static str;
}

/// Compensation action to remove melt setup and reset quote state.
///
/// This compensation is used when payment fails or finalization fails after
/// the setup transaction has committed. It removes:
/// - Input proofs (identified by input_ys)
/// - Output blinded messages (identified by blinded_secrets)
/// - Melt request tracking record
///
///   And resets:
/// - Quote state from Pending back to Unpaid
///
/// This restores the database to its pre-melt state, allowing the user to retry.
pub struct RemoveMeltSetup {
    /// Y values (public keys) from the input proofs
    pub input_ys: Vec<PublicKey>,
    /// Blinded secrets (B values) from the change output blinded messages
    pub blinded_secrets: Vec<PublicKey>,
    /// Quote ID to reset state
    pub quote_id: QuoteId,
}

#[async_trait]
impl CompensatingAction for RemoveMeltSetup {
    #[instrument(skip_all)]
    async fn execute(&self, db: &DynMintDatabase) -> Result<(), Error> {
        tracing::info!(
            "Compensation: Removing melt setup for quote {} ({} proofs, {} blinded messages)",
            self.quote_id,
            self.input_ys.len(),
            self.blinded_secrets.len()
        );

        super::super::shared::rollback_melt_quote(
            db,
            &self.quote_id,
            &self.input_ys,
            &self.blinded_secrets,
        )
        .await
    }

    fn name(&self) -> &'static str {
        "RemoveMeltSetup"
    }
}
