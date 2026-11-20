//! Per-connection prepared statement cache.

use std::collections::VecDeque;
use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::raw::RawStatement;
use crate::statement::Statement;

/// Default number of cached statements per connection.
pub(crate) const DEFAULT_CAPACITY: usize = 16;

/// Least recently used statements, keyed by trimmed SQL.
pub(crate) struct StatementCache {
    capacity: usize,
    entries: VecDeque<(String, RawStatement)>,
}

impl StatementCache {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    pub(crate) fn take(&mut self, sql: &str) -> Option<RawStatement> {
        let position = self.entries.iter().position(|(key, _)| key == sql)?;
        self.entries.remove(position).map(|(_, stmt)| stmt)
    }

    fn put(&mut self, sql: String, stmt: RawStatement) {
        // Another copy may have been returned while this one was in use.
        if self.capacity == 0 || self.entries.iter().any(|(key, _)| *key == sql) {
            return;
        }
        self.entries.push_back((sql, stmt));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.entries.len() > capacity {
            self.entries.pop_front();
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// A statement from [`Connection::prepare_cached`](crate::Connection::prepare_cached).
///
/// Dropping it resets the statement, clears its bindings and returns it to
/// the cache.
pub struct CachedStatement<'conn> {
    stmt: Option<Statement<'conn>>,
    sql: String,
}

impl fmt::Debug for CachedStatement<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedStatement")
            .field("stmt", &self.stmt)
            .finish_non_exhaustive()
    }
}

impl<'conn> CachedStatement<'conn> {
    pub(crate) fn new(stmt: Statement<'conn>, sql: String) -> Self {
        Self {
            stmt: Some(stmt),
            sql,
        }
    }

    /// Finalizes the statement instead of returning it to the cache.
    pub fn discard(mut self) {
        self.stmt = None;
    }
}

impl<'conn> Deref for CachedStatement<'conn> {
    type Target = Statement<'conn>;

    fn deref(&self) -> &Self::Target {
        self.stmt.as_ref().expect("statement is present until drop")
    }
}

impl DerefMut for CachedStatement<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.stmt.as_mut().expect("statement is present until drop")
    }
}

impl Drop for CachedStatement<'_> {
    fn drop(&mut self) {
        let Some(stmt) = self.stmt.take() else {
            return;
        };
        let conn = stmt.connection();
        let mut raw = stmt.into_raw();
        raw.reset();
        // Do not keep parameter values, which may be secrets, alive in the cache.
        raw.clear_bindings();
        if let Ok(mut cache) = conn.cache().try_borrow_mut() {
            cache.put(std::mem::take(&mut self.sql), raw);
        }
    }
}
