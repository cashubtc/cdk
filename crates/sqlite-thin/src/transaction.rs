//! Transactions.

use std::ops::Deref;

use crate::connection::Connection;
use crate::error::Result;

/// How a transaction acquires its lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionBehavior {
    /// Lock on first read or write.
    #[default]
    Deferred,
    /// Take the write lock immediately.
    Immediate,
    /// Take an exclusive lock immediately.
    Exclusive,
}

/// A transaction that rolls back on drop unless committed.
///
/// It dereferences to [`Connection`], so queries run inside it.
#[derive(Debug)]
pub struct Transaction<'conn> {
    conn: &'conn Connection,
}

impl<'conn> Transaction<'conn> {
    pub(crate) fn begin(conn: &'conn Connection, behavior: TransactionBehavior) -> Result<Self> {
        conn.execute_batch(match behavior {
            TransactionBehavior::Deferred => "BEGIN DEFERRED",
            TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
        })?;
        Ok(Self { conn })
    }

    /// Commits the transaction.
    ///
    /// If the commit fails, the transaction is rolled back when `self` drops.
    pub fn commit(self) -> Result<()> {
        self.conn.execute_batch("COMMIT")
    }

    /// Rolls the transaction back.
    pub fn rollback(self) -> Result<()> {
        self.conn.execute_batch("ROLLBACK")
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.conn.is_autocommit() {
            // Drop cannot report errors. If the rollback fails, the connection
            // stays in the transaction, and `is_autocommit` shows it.
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}
