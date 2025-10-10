use std::collections::VecDeque;
use std::sync::Arc;

use cdk_common::database::DynMintDatabase;
use cdk_common::nuts::BlindedMessage;
use cdk_common::{database, Error, Proofs, ProofsMethods, PublicKey, QuoteId, State};
use tracing::instrument;

use self::compensation::{CompensatingAction, RemoveSwapSetup};
use self::state::SwapState;
use crate::mint::subscription::PubSubManager;

mod compensation;
mod state;

/// Saga pattern implementation for atomic swap operations.
///
/// # Why Use the Saga Pattern?
///
/// The swap operation consists of multiple steps that span database transactions
/// and non-transactional operations (blind signing). We need to ensure atomicity
/// across these heterogeneous steps while maintaining consistency in failure scenarios.
///
/// Traditional ACID transactions cannot span:
/// 1. Multiple database transactions (TX1: setup, TX2: finalize)
/// 2. Non-database operations (blind signing of outputs)
///
/// The saga pattern solves this by:
/// - Breaking the operation into discrete steps with clear state transitions
/// - Recording compensating actions for each forward step
/// - Automatically rolling back via compensations if any step fails
///
/// # Transaction Boundaries
///
/// - **TX1 (setup_swap)**: Atomically verifies balance, adds input proofs (pending),
///   and adds output blinded messages
/// - **Signing (sign_outputs)**: Non-transactional cryptographic operation
/// - **TX2 (finalize)**: Atomically adds signatures to outputs and marks inputs as spent
///
/// # Expected Actions
///
/// 1. **setup_swap**: Verifies the swap is balanced, reserves inputs, prepares outputs
///    - Compensation: Removes both inputs and outputs if later steps fail
/// 2. **sign_outputs**: Performs blind signing (no DB changes)
///    - Triggers compensation if signing fails
/// 3. **finalize**: Commits signatures and marks inputs spent
///    - Triggers compensation if finalization fails
///    - Clears compensations on success (swap complete)
///
/// # Failure Handling
///
/// If any step fails after setup_swap, all compensating actions are executed in reverse
/// order to restore the database to its pre-swap state. This ensures no partial swaps
/// leave the system in an inconsistent state.
pub struct SwapSaga<'a> {
    mint: &'a super::Mint,
    db: DynMintDatabase,
    pubsub: Arc<PubSubManager>,
    state: SwapState,
    compensations: VecDeque<Box<dyn CompensatingAction>>,
}

impl<'a> SwapSaga<'a> {
    pub fn new(mint: &'a super::Mint, db: DynMintDatabase, pubsub: Arc<PubSubManager>) -> Self {
        Self {
            mint,
            db,
            pubsub,
            state: SwapState::Initial,
            compensations: VecDeque::new(),
        }
    }

    #[instrument(skip_all)]
    fn transition(&mut self, new_state: SwapState) -> Result<(), Error> {
        if !self.state.can_transition_to(&new_state) {
            tracing::error!(
                "Invalid state transition: {} -> {}",
                self.state.name(),
                new_state.name()
            );
            return Err(Error::Internal);
        }

        tracing::debug!(
            "State transition: {} -> {}",
            self.state.name(),
            new_state.name()
        );
        self.state = new_state;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn compensate_all(&mut self) -> Result<(), Error> {
        if self.compensations.is_empty() {
            return Ok(());
        }

        tracing::warn!("Running {} compensating actions", self.compensations.len());

        while let Some(compensation) = self.compensations.pop_front() {
            tracing::debug!("Running compensation: {}", compensation.name());
            if let Err(e) = compensation.execute(&self.db).await {
                tracing::error!(
                    "Compensation {} failed: {}. Continuing...",
                    compensation.name(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Sets up the swap by atomically verifying balance and reserving inputs/outputs.
    ///
    /// This is the first transaction (TX1) in the saga and must complete before blind signing.
    ///
    /// # What This Does
    ///
    /// Within a single database transaction:
    /// 1. Verifies the swap is balanced (input amount >= output amount + fees)
    /// 2. Adds input proofs to the database
    /// 3. Updates input proof states from Unspent to Pending
    /// 4. Adds output blinded messages to the database
    /// 5. Publishes proof state changes via pubsub
    ///
    /// # Compensation
    ///
    /// Registers a compensation action that will remove both the input proofs and output
    /// blinded messages if any subsequent step (signing or finalization) fails.
    ///
    /// # Errors
    ///
    /// - `TokenPending`: Proofs are already pending or blinded messages are duplicates
    /// - `TokenAlreadySpent`: Proofs have already been spent
    /// - `DuplicateOutputs`: Output blinded messages already exist
    #[instrument(skip_all)]
    pub async fn setup_swap(
        &mut self,
        input_proofs: &Proofs,
        blinded_messages: &[BlindedMessage],
        quote_id: Option<QuoteId>,
        input_verification: crate::mint::Verification,
    ) -> Result<(), Error> {
        tracing::info!("TX1: Setting up swap (verify + inputs + outputs)");

        let mut tx = self.db.begin_transaction().await?;

        // Verify balance within the transaction
        self.mint
            .verify_transaction_balanced(
                &mut tx,
                input_verification,
                input_proofs,
                blinded_messages,
            )
            .await?;

        // Add input proofs to DB
        if let Err(err) = tx.add_proofs(input_proofs.clone(), quote_id.clone()).await {
            tx.rollback().await?;
            return Err(match err {
                database::Error::Duplicate => Error::TokenPending,
                database::Error::AttemptUpdateSpentProof => Error::TokenAlreadySpent,
                _ => Error::Database(err),
            });
        }

        let ys = match input_proofs.ys() {
            Ok(ys) => ys,
            Err(err) => return Err(Error::NUT00(err)),
        };

        // Update input proof states to Pending
        let original_proof_states = match tx.update_proofs_states(&ys, State::Pending).await {
            Ok(states) => states,
            Err(database::Error::AttemptUpdateSpentProof)
            | Err(database::Error::AttemptRemoveSpentProof) => {
                tx.rollback().await?;
                return Err(Error::TokenAlreadySpent);
            }
            Err(err) => {
                tx.rollback().await?;
                return Err(err.into());
            }
        };

        // Verify proofs weren't already pending or spent
        if ys.len() != original_proof_states.len() {
            tracing::error!("Mismatched proof states");
            tx.rollback().await?;
            return Err(Error::Internal);
        }

        let forbidden_states = [State::Pending, State::Spent];
        for original_state in original_proof_states.iter().flatten() {
            if forbidden_states.contains(original_state) {
                tx.rollback().await?;
                return Err(if *original_state == State::Pending {
                    Error::TokenPending
                } else {
                    Error::TokenAlreadySpent
                });
            }
        }

        // Add output blinded messages
        if let Err(err) = tx
            .add_blinded_messages(quote_id.as_ref(), blinded_messages)
            .await
        {
            tx.rollback().await?;
            return Err(match err {
                database::Error::Duplicate => Error::DuplicateOutputs,
                _ => Error::Database(err),
            });
        }

        // Publish proof state changes
        for pk in &ys {
            self.pubsub.proof_state((*pk, State::Pending));
        }

        tx.commit().await?;

        let blinded_messages = blinded_messages.to_vec();

        // Extract secrets for compensation
        let secrets: Vec<PublicKey> = blinded_messages
            .iter()
            .map(|bm| bm.blinded_secret)
            .collect();

        // Register compensation
        self.compensations.push_front(Box::new(RemoveSwapSetup {
            secrets,
            ys: ys.clone(),
        }));

        self.transition(SwapState::SetupComplete {
            blinded_messages,
            ys,
        })?;

        Ok(())
    }

    /// Performs blind signing of output blinded messages.
    ///
    /// This is a non-transactional cryptographic operation that happens after `setup_swap`
    /// and before `finalize`. No database changes occur in this step.
    ///
    /// # What This Does
    ///
    /// 1. Retrieves blinded messages from the current state (must be in SetupComplete state)
    /// 2. Calls the mint's blind signing function to generate signatures
    /// 3. Transitions to the Signed state with the signatures
    ///
    /// # Failure Handling
    ///
    /// If blind signing fails, all registered compensations are executed to roll back
    /// the setup transaction, removing both input proofs and output blinded messages.
    ///
    /// # Errors
    ///
    /// - `Internal`: Called from an invalid state (not SetupComplete)
    /// - Propagates any errors from the blind signing operation
    #[instrument(skip_all)]
    pub async fn sign_outputs(&mut self) -> Result<(), Error> {
        tracing::info!("Signing outputs (no DB)");

        let (blinded_messages, ys) = match &self.state {
            SwapState::SetupComplete {
                blinded_messages,
                ys,
            } => (blinded_messages.clone(), ys.clone()),
            _ => {
                tracing::error!("Cannot sign from current state: {}", self.state.name());
                self.compensate_all().await?;
                return Err(Error::Internal);
            }
        };

        match self.mint.blind_sign(blinded_messages.clone()).await {
            Ok(signatures) => {
                self.transition(SwapState::Signed {
                    blinded_messages,
                    signatures,
                    ys,
                })?;
                Ok(())
            }
            Err(err) => {
                self.compensate_all().await?;
                Err(err)
            }
        }
    }

    /// Finalizes the swap by committing signatures and marking inputs as spent.
    ///
    /// This is the second and final transaction (TX2) in the saga and completes the swap.
    ///
    /// # What This Does
    ///
    /// Within a single database transaction:
    /// 1. Adds the blind signatures to the output blinded messages
    /// 2. Updates input proof states from Pending to Spent
    /// 3. Publishes proof state changes via pubsub
    /// 4. Clears all registered compensations (swap successfully completed)
    ///
    /// # Failure Handling
    ///
    /// If finalization fails, all registered compensations are executed to roll back
    /// the setup transaction, removing both input proofs and output blinded messages.
    /// The signatures are not persisted, so they are lost.
    ///
    /// # Success
    ///
    /// On success, compensations are cleared and the swap is complete. The client
    /// can now use the returned signatures to construct valid proofs.
    ///
    /// # Errors
    ///
    /// - `Internal`: Called from an invalid state (not Signed)
    /// - `TokenAlreadySpent`: Input proofs were already spent by another operation
    /// - Propagates any database errors
    #[instrument(skip_all)]
    pub async fn finalize(&mut self) -> Result<cdk_common::nuts::SwapResponse, Error> {
        tracing::info!("TX2: Finalizing swap (signatures + mark spent)");

        let (blinded_messages, signatures, ys) = match &self.state {
            SwapState::Signed {
                blinded_messages,
                signatures,
                ys,
            } => (blinded_messages.clone(), signatures.clone(), ys.clone()),
            _ => {
                tracing::error!("Cannot finalize from current state: {}", self.state.name());
                self.compensate_all().await?;
                return Err(Error::Internal);
            }
        };

        let secrets: Vec<PublicKey> = blinded_messages
            .iter()
            .map(|bm| bm.blinded_secret)
            .collect();

        let mut tx = self.db.begin_transaction().await?;

        // Add blind signatures to outputs
        if let Err(err) = tx.add_blind_signatures(&secrets, &signatures, None).await {
            tx.rollback().await?;
            self.compensate_all().await?;
            return Err(err.into());
        }

        // Mark input proofs as spent
        match tx.update_proofs_states(&ys, State::Spent).await {
            Ok(_) => {}
            Err(database::Error::AttemptUpdateSpentProof)
            | Err(database::Error::AttemptRemoveSpentProof) => {
                tx.rollback().await?;
                self.compensate_all().await?;
                return Err(Error::TokenAlreadySpent);
            }
            Err(err) => {
                tx.rollback().await?;
                self.compensate_all().await?;
                return Err(err.into());
            }
        }

        // Publish proof state changes
        for pk in &ys {
            self.pubsub.proof_state((*pk, State::Spent));
        }

        tx.commit().await?;

        // Clear compensations - swap is complete
        self.compensations.clear();

        self.transition(SwapState::Completed)?;

        Ok(cdk_common::nuts::SwapResponse::new(signatures))
    }
}
