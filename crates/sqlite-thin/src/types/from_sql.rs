//! SQLite to Rust conversions.

use std::error::Error as StdError;
use std::fmt;

use super::{Value, ValueRef};

/// Errors from [`FromSql`] conversions.
#[derive(Debug)]
#[non_exhaustive]
pub enum FromSqlError {
    /// The SQLite value has an incompatible type.
    InvalidType,
    /// The integer does not fit in the requested type.
    OutOfRange(i64),
    /// A fixed-size blob had the wrong length.
    InvalidBlobSize {
        /// The length the Rust type needs.
        expected_size: usize,
        /// The length of the blob.
        blob_size: usize,
    },
    /// Any other conversion error.
    Other(Box<dyn StdError + Send + Sync + 'static>),
}

impl fmt::Display for FromSqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidType => f.write_str("invalid type"),
            Self::OutOfRange(value) => write!(f, "value {value} out of range"),
            Self::InvalidBlobSize {
                expected_size,
                blob_size,
            } => write!(
                f,
                "cannot read {blob_size} byte blob into a {expected_size} byte array"
            ),
            Self::Other(error) => error.fmt(f),
        }
    }
}

impl StdError for FromSqlError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Other(error) => Some(&**error),
            _ => None,
        }
    }
}

/// Result alias for [`FromSql`] conversions.
pub type FromSqlResult<T> = Result<T, FromSqlError>;

/// Converts a SQLite value into a Rust value.
pub trait FromSql: Sized {
    /// Converts `value`.
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self>;
}

macro_rules! from_sql_integer {
    ($($t:ty),*) => {$(
        impl FromSql for $t {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                let value = value.as_i64()?;
                <$t>::try_from(value).map_err(|_| FromSqlError::OutOfRange(value))
            }
        }
    )*};
}

from_sql_integer!(i8, i16, i32, isize, u8, u16, u32, u64, usize);

impl FromSql for i64 {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_i64()
    }
}

impl FromSql for bool {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_i64().map(|value| value != 0)
    }
}

impl FromSql for f64 {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Integer(value) => Ok(value as f64),
            ValueRef::Real(value) => Ok(value),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

impl FromSql for f32 {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        f64::column_result(value).map(|value| value as f32)
    }
}

impl FromSql for String {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str().map(ToOwned::to_owned)
    }
}

impl FromSql for Vec<u8> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_bytes().map(<[u8]>::to_vec)
    }
}

impl<const N: usize> FromSql for [u8; N] {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let blob = value.as_blob()?;
        blob.try_into().map_err(|_| FromSqlError::InvalidBlobSize {
            expected_size: N,
            blob_size: blob.len(),
        })
    }
}

impl<T> FromSql for Option<T>
where
    T: FromSql,
{
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Null => Ok(None),
            value => T::column_result(value).map(Some),
        }
    }
}

impl FromSql for Value {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        Ok(match value {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => Self::Real(value),
            ValueRef::Text(_) => Self::Text(value.as_str()?.to_owned()),
            ValueRef::Blob(blob) => Self::Blob(blob.to_vec()),
        })
    }
}
