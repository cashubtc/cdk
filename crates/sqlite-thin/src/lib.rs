//! A small, safe SQLite wrapper.
//!
//! The API is a subset of [rusqlite](https://docs.rs/rusqlite)'s, so porting
//! code is mostly a matter of changing imports. What differs:
//!
//! - **Any `libsqlite3-sys` version.** The C functions this crate calls are
//!   declared by hand, so `libsqlite3-sys` is only used to build and link
//!   SQLite. Any version works, and this crate never forces one on the other
//!   SQLite users in the build.
//! - **Small surface.** Connections, statements, a prepared-statement cache,
//!   transactions, pragmas and value conversions. No hooks, functions, virtual
//!   tables, backups or blobs.
//! - **One unsafe module.** Every call into SQLite lives in `raw`; everything
//!   else is safe Rust checked by the compiler.
//! - [`Rows::next`] returns a [`Row`] by value rather than by reference.
//!
//! ```
//! use sqlite_thin::{params, Connection};
//!
//! # fn main() -> sqlite_thin::Result<()> {
//! let conn = Connection::open_in_memory()?;
//! conn.execute_batch("CREATE TABLE kv (k TEXT PRIMARY KEY, v BLOB)")?;
//! conn.execute(
//!     "INSERT INTO kv (k, v) VALUES (?1, ?2)",
//!     params!["a", vec![1u8, 2]],
//! )?;
//! let v: Vec<u8> = conn.query_row("SELECT v FROM kv WHERE k = ?1", ["a"], |row| row.get(0))?;
//! assert_eq!(v, [1, 2]);
//! # Ok(())
//! # }
//! ```

// Linked for the SQLite library it builds; none of its Rust items are used.
extern crate libsqlite3_sys as _;

mod cache;
mod connection;
mod error;
pub mod ffi;
mod params;
mod raw;
mod row;
mod statement;
mod sys;
mod transaction;
pub mod types;

pub use self::cache::CachedStatement;
pub use self::connection::{Connection, DatabaseName, OpenFlags};
pub use self::error::{Error, ErrorCode, OptionalExtension, Result, SqliteError};
pub use self::params::{params_from_iter, Params, ParamsFromIter};
pub use self::row::{MappedRows, Row, RowIndex, Rows};
pub use self::statement::Statement;
pub use self::transaction::{Transaction, TransactionBehavior};
pub use self::types::ToSql;

/// Builds positional parameters from a list of [`ToSql`] expressions.
///
/// ```
/// # use sqlite_thin::{params, Connection};
/// # let conn = Connection::open_in_memory().expect("open");
/// conn.execute("SELECT ?1, ?2", params![1, "two"]).ok();
/// ```
#[macro_export]
macro_rules! params {
    () => {
        &[] as &[&dyn $crate::ToSql]
    };
    ($($param:expr),+ $(,)?) => {
        &[$(&$param as &dyn $crate::ToSql),+] as &[&dyn $crate::ToSql]
    };
}

/// Builds named parameters from `"name": value` pairs.
///
/// The name includes its prefix character, as written in the SQL.
///
/// ```
/// # use sqlite_thin::{named_params, Connection};
/// # let conn = Connection::open_in_memory().expect("open");
/// conn.execute("SELECT :a, :b", named_params! { ":a": 1, ":b": "two" })
///     .ok();
/// ```
#[macro_export]
macro_rules! named_params {
    () => {
        &[] as &[(&str, &dyn $crate::ToSql)]
    };
    ($($name:literal: $value:expr),+ $(,)?) => {
        &[$(($name, &$value as &dyn $crate::ToSql)),+] as &[(&str, &dyn $crate::ToSql)]
    };
}

#[cfg(test)]
mod tests;
