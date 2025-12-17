//! Row locking mechanism for database transactions.
//!
//! This module provides a mechanism for database layers to track which rows are currently
//! locked within a transaction. The primary advantage is ensuring that upper layers always
//! read the latest state from the database and properly lock resources before modifications.
//!
//! By requiring explicit locking before updates, this prevents race conditions and ensures
//! data consistency when multiple operations might attempt to modify the same resources
//! concurrently.

use std::collections::HashSet;

use cashu::quote_id::QuoteId;
use cashu::PublicKey;

use crate::database::Error;

/// Identifies a database row that can be locked.
///
/// This enum represents the different types of resources that can be locked
/// during a database transaction, allowing for type-safe tracking of locked rows.
#[derive(Debug, Hash, Eq, PartialEq)]
pub enum RowId {
    /// A proof identified by its public key.
    Proof(PublicKey),
    /// A quote identified by its quote ID.
    Quote(QuoteId),
}

impl From<PublicKey> for RowId {
    fn from(value: PublicKey) -> Self {
        RowId::Proof(value)
    }
}

impl From<&PublicKey> for RowId {
    fn from(value: &PublicKey) -> Self {
        RowId::Proof(*value)
    }
}

impl From<&QuoteId> for RowId {
    fn from(value: &QuoteId) -> Self {
        RowId::Quote(value.to_owned())
    }
}

/// Tracks which rows are currently locked within a transaction.
///
/// This structure maintains a set of locked row identifiers, allowing the database
/// layer to verify that rows have been properly locked before allowing modifications.
/// This ensures that:
///
/// - Resources are read from the database before being modified (forcing fresh reads)
/// - Multiple concurrent operations cannot modify the same resource simultaneously
/// - Updates to unlocked rows are rejected, preventing accidental data corruption
#[derive(Debug, Default)]
pub struct LockedRows {
    inner: HashSet<RowId>,
}

impl LockedRows {
    /// Locks a single row, marking it as acquired for modification.
    ///
    /// After locking, any subsequent calls to [`is_locked`](Self::is_locked) for this
    /// row will succeed. This should be called when reading a row that will be modified.
    pub fn lock<T>(&mut self, record_id: T)
    where
        T: Into<RowId>,
    {
        self.inner.insert(record_id.into());
    }

    /// Locks multiple rows at once.
    ///
    /// This is a convenience method equivalent to calling [`lock`](Self::lock)
    /// for each item in the collection.
    pub fn lock_many<T>(&mut self, records_id: Vec<T>)
    where
        T: Into<RowId>,
    {
        records_id.into_iter().for_each(|record_id| {
            self.inner.insert(record_id.into());
        });
    }

    /// Verifies that all specified rows are currently locked.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UpdatingUnlockedRecord`] if any of the specified rows
    /// have not been locked. This prevents modifications to rows that weren't
    /// properly read first.
    pub fn is_locked_many<T>(&self, records_id: Vec<T>) -> Result<(), Error>
    where
        T: Into<RowId>,
    {
        records_id.into_iter().try_for_each(|resource_id| {
            let id = resource_id.into();
            self.inner
                .contains(&id)
                .then_some(())
                .ok_or(Error::UpdatingUnlockedRecord(id))
        })
    }

    /// Verifies that a single row is currently locked.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UpdatingUnlockedRecord`] if the specified row has not
    /// been locked. This prevents modifications to rows that weren't properly
    /// read first.
    pub fn is_locked<T>(&self, resource_id: T) -> Result<(), Error>
    where
        T: Into<RowId>,
    {
        let id = resource_id.into();
        self.inner
            .contains(&id)
            .then_some(())
            .ok_or(Error::UpdatingUnlockedRecord(id))
    }
}
