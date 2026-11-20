//! Statements already parsed on one PostgreSQL connection.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio_postgres::Statement as PgStatement;

/// CDK issues a fixed set of SQL literals, so this only bounds an unforeseen source of
/// generated SQL; reaching it re-parses rather than evicting an entry that is still hot.
const MAX_CACHED_STATEMENTS: usize = 512;

/// Statements parsed on a single connection, keyed by their rendered SQL.
///
/// Sound only because the pool returns connections untouched: a `DISCARD ALL` reset would
/// deallocate these server-side and leave every entry naming a statement that no longer exists.
#[derive(Default)]
pub struct StatementCache {
    entries: Mutex<HashMap<String, PgStatement>>,
    warned_full: AtomicBool,
}

impl StatementCache {
    /// Recovers a poisoned lock, since the map holds no invariant a panic could leave broken.
    fn entries(&self) -> MutexGuard<'_, HashMap<String, PgStatement>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The statement already parsed for `sql` on this connection, if there is one.
    pub fn get(&self, sql: &str) -> Option<PgStatement> {
        self.entries().get(sql).cloned()
    }

    /// Remembers `statement` so later callers skip its Parse round trip.
    pub fn insert(&self, sql: &str, statement: &PgStatement) {
        let mut entries = self.entries();

        if entries.len() >= MAX_CACHED_STATEMENTS && !entries.contains_key(sql) {
            if !self.warned_full.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "Prepared statement cache is full at {} entries; further statements are \
                     re-parsed on every call",
                    MAX_CACHED_STATEMENTS
                );
            }
            return;
        }

        entries.insert(sql.to_owned(), statement.clone());
    }

    /// Forgets `sql` so the next caller parses it again against the current schema.
    pub fn remove(&self, sql: &str) {
        self.entries().remove(sql);
    }

    /// Forgets every statement, for use once DDL may have invalidated their plans.
    pub fn clear(&self) {
        self.entries().clear();
    }
}

impl fmt::Debug for StatementCache {
    /// Reports the entry count, because the keys are every SQL statement the process issues.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementCache")
            .field("entries", &self.entries().len())
            .finish()
    }
}
