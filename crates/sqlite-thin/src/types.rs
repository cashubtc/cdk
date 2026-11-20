//! Conversions between Rust values and SQLite values.

use std::fmt;

mod from_sql;
mod to_sql;
mod value;
mod value_ref;

pub use self::from_sql::{FromSql, FromSqlError, FromSqlResult};
pub use self::to_sql::{ToSql, ToSqlOutput};
pub use self::value::Value;
pub use self::value_ref::ValueRef;

/// SQLite storage classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// NULL
    Null,
    /// 64-bit signed integer
    Integer,
    /// 64-bit IEEE float
    Real,
    /// Text string
    Text,
    /// Binary blob
    Blob,
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Null => "Null",
            Self::Integer => "Integer",
            Self::Real => "Real",
            Self::Text => "Text",
            Self::Blob => "Blob",
        })
    }
}

/// Binds SQL NULL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Null;
