//! Borrowed SQLite values.

use super::{FromSqlError, FromSqlResult, Type, Value};

/// A borrowed SQLite value.
///
/// Text is kept as bytes because SQLite does not validate UTF-8;
/// [`as_str`](Self::as_str) checks it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueRef<'a> {
    /// NULL
    Null,
    /// 64-bit signed integer
    Integer(i64),
    /// 64-bit IEEE float
    Real(f64),
    /// Text bytes, normally UTF-8
    Text(&'a [u8]),
    /// Binary blob
    Blob(&'a [u8]),
}

impl<'a> ValueRef<'a> {
    /// The storage class of this value.
    pub fn data_type(&self) -> Type {
        match self {
            Self::Null => Type::Null,
            Self::Integer(_) => Type::Integer,
            Self::Real(_) => Type::Real,
            Self::Text(_) => Type::Text,
            Self::Blob(_) => Type::Blob,
        }
    }

    /// The integer, or [`FromSqlError::InvalidType`].
    pub fn as_i64(&self) -> FromSqlResult<i64> {
        match self {
            Self::Integer(value) => Ok(*value),
            _ => Err(FromSqlError::InvalidType),
        }
    }

    /// The float, or [`FromSqlError::InvalidType`].
    pub fn as_f64(&self) -> FromSqlResult<f64> {
        match self {
            Self::Real(value) => Ok(*value),
            _ => Err(FromSqlError::InvalidType),
        }
    }

    /// The text as `&str`, or an error for other types and invalid UTF-8.
    pub fn as_str(&self) -> FromSqlResult<&'a str> {
        match self {
            Self::Text(text) => {
                std::str::from_utf8(text).map_err(|error| FromSqlError::Other(Box::new(error)))
            }
            _ => Err(FromSqlError::InvalidType),
        }
    }

    /// The blob, or [`FromSqlError::InvalidType`].
    pub fn as_blob(&self) -> FromSqlResult<&'a [u8]> {
        match self {
            Self::Blob(blob) => Ok(blob),
            _ => Err(FromSqlError::InvalidType),
        }
    }

    /// The raw bytes of a text or blob value.
    pub fn as_bytes(&self) -> FromSqlResult<&'a [u8]> {
        match self {
            Self::Text(bytes) | Self::Blob(bytes) => Ok(bytes),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

impl<'a> From<&'a str> for ValueRef<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(value.as_bytes())
    }
}

impl<'a> From<&'a [u8]> for ValueRef<'a> {
    fn from(value: &'a [u8]) -> Self {
        Self::Blob(value)
    }
}

impl<'a> From<&'a Value> for ValueRef<'a> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Integer(value) => Self::Integer(*value),
            Value::Real(value) => Self::Real(*value),
            Value::Text(text) => Self::Text(text.as_bytes()),
            Value::Blob(blob) => Self::Blob(blob),
        }
    }
}
