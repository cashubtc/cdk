//! Proof writer
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cdk_common::database::{self, MintDatabase, MintTransaction};
use cdk_common::{Error, Proofs, ProofsMethods, PublicKey, State};

use super::subscription::PubSubManager;

type Db = Arc<dyn MintDatabase<database::Error> + Send + Sync>;
type Tx<'a, 'b> = Box<dyn MintTransaction<'a, database::Error> + Send + Sync + 'b>;

/// Proof writer
///
/// This is a proof writer that emulates a database transaction but without holding the transaction
/// alive while waiting for external events to be fully committed to the database; instead, it
/// maintains a `pending` state.
///
/// This struct allows for premature exit on error, enabling it to remove proofs or reset their
/// status.
///
/// This struct is not fully ACID. If the process exits due to a panic, and the `Drop` function
/// cannot be run, the reset process should reset the state.
pub struct ProofWriter {
    db: Option<Db>,
    pubsub_manager: Arc<PubSubManager>,
    proof_original_states: Option<HashMap<PublicKey, Option<State>>>,
}

impl ProofWriter {
    /// Creates a new ProofWriter on top of the database
    pub fn new(db: Db, pubsub_manager: Arc<PubSubManager>) -> Self {
        Self {
            db: Some(db),
            pubsub_manager,
            proof_original_states: Some(Default::default()),
        }
    }

    /// The changes are permanent, consume the struct removing the database, so the Drop does
    /// nothing
    pub fn commit(mut self) {
        self.db.take();
        self.proof_original_states.take();
    }

    /// Add proofs
    pub async fn add_proofs(
        &mut self,
        tx: &mut Tx<'_, '_>,
        proofs: &Proofs,
    ) -> Result<Vec<PublicKey>, Error> {
        let proof_states = if let Some(proofs) = self.proof_original_states.as_mut() {
            proofs
        } else {
            return Err(Error::Internal);
        };

        if let Some(err) = tx.add_proofs(proofs.clone(), None).await.err() {
            return match err {
                cdk_common::database::Error::Duplicate => Err(Error::TokenPending),
                cdk_common::database::Error::AttemptUpdateSpentProof => {
                    Err(Error::TokenAlreadySpent)
                }
                err => Err(Error::Database(err)),
            };
        }

        let ys = proofs.ys()?;

        for pk in ys.iter() {
            proof_states.insert(*pk, None);
        }

        self.update_proofs_states(tx, &ys, State::Pending).await?;

        Ok(ys)
    }

    /// Update proof status
    pub async fn update_proofs_states(
        &mut self,
        tx: &mut Tx<'_, '_>,
        ys: &[PublicKey],
        new_proof_state: State,
    ) -> Result<(), Error> {
        let proof_states = if let Some(proofs) = self.proof_original_states.as_mut() {
            proofs
        } else {
            return Err(Error::Internal);
        };

        let original_proofs_state = match tx.update_proofs_states(ys, new_proof_state).await {
            Ok(states) => states,
            Err(database::Error::AttemptUpdateSpentProof)
            | Err(database::Error::AttemptRemoveSpentProof) => {
                return Err(Error::TokenAlreadySpent)
            }
            Err(err) => return Err(err.into()),
        };

        if ys.len() != original_proofs_state.len() {
            return Err(Error::Internal);
        }

        let proofs_state = original_proofs_state
            .iter()
            .flatten()
            .map(|x| x.to_owned())
            .collect::<HashSet<State>>();

        let forbidden_states = if new_proof_state == State::Pending {
            // If the new state is `State::Pending` it cannot be pending already
            vec![State::Pending, State::Spent]
        } else {
            // For other state it cannot be spent
            vec![State::Spent]
        };

        for forbidden_state in forbidden_states.iter() {
            if proofs_state.contains(forbidden_state) {
                reset_proofs_to_original_state(tx, ys, original_proofs_state).await?;

                return Err(if proofs_state.contains(&State::Pending) {
                    Error::TokenPending
                } else {
                    Error::TokenAlreadySpent
                });
            }
        }

        for (idx, ys) in ys.iter().enumerate() {
            proof_states
                .entry(*ys)
                .or_insert(original_proofs_state[idx]);
        }

        for pk in ys {
            self.pubsub_manager.proof_state((*pk, new_proof_state));
        }

        Ok(())
    }

    /// Rollback all changes in this ProofWriter consuming it.
    pub async fn rollback(mut self) -> Result<(), Error> {
        let db = if let Some(db) = self.db.take() {
            db
        } else {
            return Ok(());
        };
        let mut tx = db.begin_transaction().await?;
        let (ys, original_states) = if let Some(proofs) = self.proof_original_states.take() {
            proofs.into_iter().unzip::<_, _, Vec<_>, Vec<_>>()
        } else {
            return Ok(());
        };

        tracing::info!(
            "Rollback {} proofs to their original states {:?}",
            ys.len(),
            original_states
        );

        reset_proofs_to_original_state(&mut tx, &ys, original_states).await?;
        tx.commit().await?;

        Ok(())
    }
}

/// Resets proofs to their original states or removes them
#[inline(always)]
async fn reset_proofs_to_original_state(
    tx: &mut Tx<'_, '_>,
    ys: &[PublicKey],
    original_states: Vec<Option<State>>,
) -> Result<(), Error> {
    let mut ys_by_state = HashMap::new();
    let mut unknown_proofs = Vec::new();
    for (y, state) in ys.iter().zip(original_states) {
        if let Some(state) = state {
            // Skip attempting to update proofs that were originally spent
            if state != State::Spent {
                ys_by_state.entry(state).or_insert_with(Vec::new).push(*y);
            }
        } else {
            unknown_proofs.push(*y);
        }
    }

    for (state, ys) in ys_by_state {
        tx.update_proofs_states(&ys, state).await?;
    }

    if !unknown_proofs.is_empty() {
        tx.remove_proofs(&unknown_proofs, None).await?;
    }

    Ok(())
}

#[inline(always)]
async fn rollback(
    db: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
    ys: Vec<PublicKey>,
    original_states: Vec<Option<State>>,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    reset_proofs_to_original_state(&mut tx, &ys, original_states).await?;
    tx.commit().await?;

    Ok(())
}

impl Drop for ProofWriter {
    fn drop(&mut self) {
        let db = if let Some(db) = self.db.take() {
            db
        } else {
            return;
        };
        let (ys, states) = if let Some(proofs) = self.proof_original_states.take() {
            proofs.into_iter().unzip()
        } else {
            return;
        };

        tokio::spawn(rollback(db, ys, states));
    }
}
