//! Database connections.

use std::cell::RefCell;
use std::ffi::{c_int, CString};
use std::fmt::{self, Write as _};
use std::ops::BitOr;
use std::path::Path;
use std::time::Duration;

use crate::cache::{CachedStatement, StatementCache, DEFAULT_CAPACITY};
use crate::error::{Error, Result, SqliteError};
use crate::ffi;
use crate::params::Params;
use crate::raw::RawConnection;
use crate::row::Row;
use crate::statement::Statement;
use crate::transaction::{Transaction, TransactionBehavior};
use crate::types::{ToSql, ValueRef};

/// How long a new connection waits for locks, matching rusqlite.
const DEFAULT_BUSY_TIMEOUT_MS: c_int = 5000;

/// Flags for [`Connection::open_with_flags`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpenFlags(c_int);

impl OpenFlags {
    /// Open read-only.
    pub const SQLITE_OPEN_READ_ONLY: Self = Self(ffi::SQLITE_OPEN_READONLY);
    /// Open for reading and writing.
    pub const SQLITE_OPEN_READ_WRITE: Self = Self(ffi::SQLITE_OPEN_READWRITE);
    /// Create the database if it does not exist.
    pub const SQLITE_OPEN_CREATE: Self = Self(ffi::SQLITE_OPEN_CREATE);
    /// Interpret the path as a URI.
    pub const SQLITE_OPEN_URI: Self = Self(ffi::SQLITE_OPEN_URI);
    /// Open an in-memory database.
    pub const SQLITE_OPEN_MEMORY: Self = Self(ffi::SQLITE_OPEN_MEMORY);
    /// Multi-thread mode: no per-connection mutex.
    pub const SQLITE_OPEN_NO_MUTEX: Self = Self(ffi::SQLITE_OPEN_NOMUTEX);
    /// Serialized mode: a per-connection mutex.
    pub const SQLITE_OPEN_FULL_MUTEX: Self = Self(ffi::SQLITE_OPEN_FULLMUTEX);
    /// Use the shared cache.
    pub const SQLITE_OPEN_SHARED_CACHE: Self = Self(ffi::SQLITE_OPEN_SHAREDCACHE);
    /// Use a private cache.
    pub const SQLITE_OPEN_PRIVATE_CACHE: Self = Self(ffi::SQLITE_OPEN_PRIVATECACHE);

    /// The raw flag bits.
    pub fn bits(self) -> c_int {
        self.0
    }

    /// Whether every flag in `other` is set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl Default for OpenFlags {
    /// Read-write, create, URI paths, no per-connection mutex.
    fn default() -> Self {
        Self::SQLITE_OPEN_READ_WRITE
            | Self::SQLITE_OPEN_CREATE
            | Self::SQLITE_OPEN_URI
            | Self::SQLITE_OPEN_NO_MUTEX
    }
}

impl BitOr for OpenFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// A database schema name, for pragmas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatabaseName<'a> {
    /// The main database.
    Main,
    /// The temporary database.
    Temp,
    /// An attached database.
    Attached(&'a str),
}

/// A SQLite database connection.
///
/// A connection is `Send` but not `Sync`: move it between threads, or share
/// it behind a mutex.
pub struct Connection {
    // Field order matters: cached statements must be finalized before the
    // connection handle closes.
    cache: RefCell<StatementCache>,
    raw: RawConnection,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("autocommit", &self.is_autocommit())
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// Opens or creates the database at `path` with default flags.
    ///
    /// Like rusqlite, new connections wait up to five seconds for locks; see
    /// [`busy_timeout`](Self::busy_timeout).
    pub fn open<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        Self::open_with_flags(path, OpenFlags::default())
    }

    /// Opens a new in-memory database.
    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Opens the database at `path` with explicit flags.
    pub fn open_with_flags<P>(path: P, flags: OpenFlags) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = path_to_cstring(path.as_ref())?;
        let raw = RawConnection::open(&path, flags.bits())?;
        raw.busy_timeout(DEFAULT_BUSY_TIMEOUT_MS)?;
        Ok(Self {
            cache: RefCell::new(StatementCache::new(DEFAULT_CAPACITY)),
            raw,
        })
    }

    pub(crate) fn cache(&self) -> &RefCell<StatementCache> {
        &self.cache
    }

    /// Prepares a single SQL statement.
    ///
    /// Fails with [`Error::MultipleStatement`] if `sql` holds more than one.
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        let (stmt, consumed) = self.raw.prepare(sql)?;
        let stmt = stmt.ok_or_else(|| {
            Error::SqliteFailure(
                SqliteError::new(ffi::SQLITE_MISUSE),
                Some("SQL contains no statement".to_owned()),
            )
        })?;
        let rest = sql.get(consumed..).unwrap_or_default();
        if !rest.trim().is_empty() {
            // Anything but comments after the first statement is rejected
            // rather than silently ignored.
            match self.raw.prepare(rest) {
                Ok((None, _)) => {}
                Ok((Some(_), _)) | Err(_) => return Err(Error::MultipleStatement),
            }
        }
        Ok(Statement::new(self, stmt))
    }

    /// Prepares a statement, reusing a cached one when available.
    pub fn prepare_cached(&self, sql: &str) -> Result<CachedStatement<'_>> {
        let key = sql.trim();
        let cached = self.cache.borrow_mut().take(key);
        let stmt = match cached {
            Some(raw) => Statement::new(self, raw),
            None => self.prepare(key)?,
        };
        Ok(CachedStatement::new(stmt, key.to_owned()))
    }

    /// Sets how many statements [`prepare_cached`](Self::prepare_cached)
    /// keeps.
    pub fn set_prepared_statement_cache_capacity(&self, capacity: usize) {
        self.cache.borrow_mut().set_capacity(capacity);
    }

    /// Finalizes every cached statement.
    pub fn flush_prepared_statement_cache(&self) {
        self.cache.borrow_mut().clear();
    }

    /// Runs one statement and returns the number of changed rows.
    pub fn execute<P>(&self, sql: &str, params: P) -> Result<usize>
    where
        P: Params,
    {
        self.prepare(sql)?.execute(params)
    }

    /// Runs every statement in `sql`, discarding any rows they return.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let mut rest = sql;
        while !rest.is_empty() {
            let (stmt, consumed) = self.raw.prepare(rest)?;
            if let Some(mut stmt) = stmt {
                let mut stepped = stmt.step();
                while let Ok(true) = stepped {
                    stepped = stmt.step();
                }
                stepped?;
            }
            if consumed == 0 {
                break;
            }
            rest = rest.get(consumed..).unwrap_or_default();
        }
        Ok(())
    }

    /// Runs a query and maps its first row with `f`.
    ///
    /// Fails with [`Error::QueryReturnedNoRows`] when there is no row.
    pub fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.prepare(sql)?.query_row(params, f)
    }

    /// Begins a deferred transaction.
    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        Transaction::begin(self, TransactionBehavior::Deferred)
    }

    /// Begins a transaction with the given locking behavior.
    pub fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        Transaction::begin(self, behavior)
    }

    /// Begins a deferred transaction through a shared reference.
    ///
    /// Unlike [`transaction`](Self::transaction), the borrow checker cannot
    /// stop a nested `BEGIN`; SQLite reports that as an error.
    pub fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        Transaction::begin(self, TransactionBehavior::Deferred)
    }

    /// Whether no transaction is open.
    pub fn is_autocommit(&self) -> bool {
        self.raw.is_autocommit()
    }

    /// Rows changed by the most recent INSERT, UPDATE or DELETE.
    pub fn changes(&self) -> u64 {
        self.raw.changes()
    }

    /// Rowid of the most recent successful INSERT.
    pub fn last_insert_rowid(&self) -> i64 {
        self.raw.last_insert_rowid()
    }

    /// Waits up to `timeout` for locks held by other connections.
    pub fn busy_timeout(&self, timeout: Duration) -> Result<()> {
        let milliseconds = c_int::try_from(timeout.as_millis()).unwrap_or(c_int::MAX);
        self.raw.busy_timeout(milliseconds)
    }

    /// Runs `PRAGMA schema.name = value`.
    ///
    /// Pragmas cannot take bound parameters, so `value` is written into the
    /// SQL as a quoted literal.
    pub fn pragma_update<V>(
        &self,
        schema: Option<DatabaseName<'_>>,
        name: &str,
        value: V,
    ) -> Result<()>
    where
        V: ToSql,
    {
        let mut sql = pragma_prefix(schema, name)?;
        sql.push_str(" = ");
        push_literal(&mut sql, value.to_sql()?.as_value_ref())?;
        self.execute_batch(&sql)
    }

    /// Runs `PRAGMA schema.name` and maps its single row with `f`.
    pub fn pragma_query_value<T, F>(
        &self,
        schema: Option<DatabaseName<'_>>,
        name: &str,
        f: F,
    ) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let sql = pragma_prefix(schema, name)?;
        self.query_row(&sql, [], f)
    }

    /// Runs `PRAGMA schema.name(value)` and calls `f` for each row.
    pub fn pragma<V, F>(
        &self,
        schema: Option<DatabaseName<'_>>,
        name: &str,
        value: V,
        mut f: F,
    ) -> Result<()>
    where
        V: ToSql,
        F: FnMut(&Row<'_>) -> Result<()>,
    {
        let mut sql = pragma_prefix(schema, name)?;
        sql.push('(');
        push_literal(&mut sql, value.to_sql()?.as_value_ref())?;
        sql.push(')');
        let mut stmt = self.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            f(&row)?;
        }
        Ok(())
    }
}

fn path_to_cstring(path: &Path) -> Result<CString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(CString::new(path.as_os_str().as_bytes())?)
    }
    #[cfg(not(unix))]
    {
        let path = path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(path.to_owned()))?;
        Ok(CString::new(path)?)
    }
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn pragma_prefix(schema: Option<DatabaseName<'_>>, name: &str) -> Result<String> {
    if !is_identifier(name) {
        return Err(Error::SqliteFailure(
            SqliteError::new(ffi::SQLITE_MISUSE),
            Some(format!("invalid pragma name: {name}")),
        ));
    }
    let mut sql = "PRAGMA ".to_owned();
    match schema {
        None => {}
        Some(DatabaseName::Main) => sql.push_str("main."),
        Some(DatabaseName::Temp) => sql.push_str("temp."),
        Some(DatabaseName::Attached(schema)) => {
            sql.push('"');
            sql.push_str(&schema.replace('"', "\"\""));
            sql.push_str("\".");
        }
    }
    sql.push_str(name);
    Ok(sql)
}

fn push_literal(sql: &mut String, value: ValueRef<'_>) -> Result<()> {
    let invalid = |reason: &str| {
        Error::SqliteFailure(
            SqliteError::new(ffi::SQLITE_MISUSE),
            Some(format!("cannot write pragma value as a literal: {reason}")),
        )
    };
    match value {
        ValueRef::Null => sql.push_str("NULL"),
        ValueRef::Integer(value) => sql.push_str(&value.to_string()),
        ValueRef::Real(value) if !value.is_finite() => return Err(invalid("non-finite float")),
        ValueRef::Real(value) => sql.push_str(&value.to_string()),
        ValueRef::Text(text) => {
            let text = std::str::from_utf8(text)?;
            if text.contains('\0') {
                return Err(invalid("text contains NUL"));
            }
            sql.push('\'');
            sql.push_str(&text.replace('\'', "''"));
            sql.push('\'');
        }
        ValueRef::Blob(blob) => {
            sql.push_str("X'");
            for byte in blob {
                // Writing to a String cannot fail.
                let _ = write!(sql, "{byte:02X}");
            }
            sql.push('\'');
        }
    }
    Ok(())
}
