//! Blinded message writer
use std::collections::HashSet;

use cdk_common::database::{self, DynMintDatabase, MintTransaction};
use cdk_common::nuts::BlindedMessage;
use cdk_common::{Error, PublicKey, QuoteId};

type Tx<'a, 'b> = Box<dyn MintTransaction<'a, database::Error> + Send + Sync + 'b>;

/// Blinded message writer
///
/// This is a blinded message writer that emulates a database transaction but without holding the
/// transaction alive while waiting for external events to be fully committed to the database;
/// instead, it maintains a `pending` state.
///
/// This struct allows for premature exit on error, enabling it to remove blinded messages that
/// were added during the operation.
///
/// This struct is not fully ACID. If the process exits due to a panic, and the `Drop` function
/// cannot be run, the cleanup process should reset the state.
pub struct BlindedMessageWriter {
    db: Option<DynMintDatabase>,
    added_blinded_secrets: Option<HashSet<PublicKey>>,
}

impl BlindedMessageWriter {
    /// Creates a new BlindedMessageWriter on top of the database
    pub fn new(db: DynMintDatabase) -> Self {
        Self {
            db: Some(db),
            added_blinded_secrets: Some(Default::default()),
        }
    }

    /// The changes are permanent, consume the struct removing the database, so the Drop does
    /// nothing
    pub fn commit(mut self) {
        self.db.take();
        self.added_blinded_secrets.take();
    }

    /// Add blinded messages
    pub async fn add_blinded_messages(
        &mut self,
        tx: &mut Tx<'_, '_>,
        quote_id: Option<QuoteId>,
        blinded_messages: &[BlindedMessage],
    ) -> Result<Vec<PublicKey>, Error> {
        let added_secrets = if let Some(secrets) = self.added_blinded_secrets.as_mut() {
            secrets
        } else {
            return Err(Error::Internal);
        };

        if let Some(err) = tx
            .add_blinded_messages(quote_id.as_ref(), blinded_messages)
            .await
            .err()
        {
            return match err {
                cdk_common::database::Error::Duplicate => Err(Error::DuplicateOutputs),
                err => Err(Error::Database(err)),
            };
        }

        let blinded_secrets: Vec<PublicKey> = blinded_messages
            .iter()
            .map(|bm| bm.blinded_secret)
            .collect();

        for blinded_secret in &blinded_secrets {
            added_secrets.insert(*blinded_secret);
        }

        Ok(blinded_secrets)
    }

    /// Rollback all changes in this BlindedMessageWriter consuming it.
    pub async fn rollback(mut self) -> Result<(), Error> {
        let db = if let Some(db) = self.db.take() {
            db
        } else {
            return Ok(());
        };
        let mut tx = db.begin_transaction().await?;
        let blinded_secrets: Vec<PublicKey> =
            if let Some(secrets) = self.added_blinded_secrets.take() {
                secrets.into_iter().collect()
            } else {
                return Ok(());
            };

        if !blinded_secrets.is_empty() {
            tracing::info!("Rollback {} blinded messages", blinded_secrets.len(),);

            remove_blinded_messages(&mut tx, &blinded_secrets).await?;
        }

        tx.commit().await?;

        Ok(())
    }
}

/// Removes blinded messages from the database
#[inline(always)]
async fn remove_blinded_messages(
    tx: &mut Tx<'_, '_>,
    blinded_secrets: &[PublicKey],
) -> Result<(), Error> {
    tx.delete_blinded_messages(blinded_secrets)
        .await
        .map_err(Error::Database)
}

#[inline(always)]
async fn rollback_blinded_messages(
    db: DynMintDatabase,
    blinded_secrets: Vec<PublicKey>,
) -> Result<(), Error> {
    let mut tx = db.begin_transaction().await?;
    remove_blinded_messages(&mut tx, &blinded_secrets).await?;
    tx.commit().await?;

    Ok(())
}

impl Drop for BlindedMessageWriter {
    fn drop(&mut self) {
        let db = if let Some(db) = self.db.take() {
            db
        } else {
            tracing::debug!("Blinded message writer dropped after commit, no need to rollback.");
            return;
        };
        let blinded_secrets: Vec<PublicKey> =
            if let Some(secrets) = self.added_blinded_secrets.take() {
                secrets.into_iter().collect()
            } else {
                return;
            };

        if !blinded_secrets.is_empty() {
            tracing::debug!("Blinded message writer dropper with messages attempting to remove.");
            tokio::spawn(async move {
                if let Err(err) = rollback_blinded_messages(db, blinded_secrets).await {
                    tracing::error!("Failed to rollback blinded messages in Drop: {}", err);
                }
            });
        }
    }
}
