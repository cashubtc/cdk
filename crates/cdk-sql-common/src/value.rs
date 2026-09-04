//! Generic Rust value representation for data from the database
use cdk_common::database::ConversionError;

/// Generic Value representation of data from the any database
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The value is a `NULL` value.
    Null,
    /// The value is a signed integer.
    Integer(i64),
    /// The value is an unsigned integer that has not been narrowed yet.
    ///
    /// Every integer column is signed, so a `u64` has no lossless
    /// representation. Conversion into [`Value`] has to be infallible, so the
    /// value is carried unchanged until the statement is rendered, where the
    /// placeholder name is known and an out of range value can be named in the
    /// error.
    Unsigned(u64),
    /// The value is a floating point number.
    Real(f64),
    /// The value is a text string.
    Text(String),
    /// The value is a blob of data
    Blob(Vec<u8>),
}

impl Value {
    /// Narrows an unsigned value onto the signed integer every column stores.
    ///
    /// `column` only names the placeholder in the error.
    pub fn into_signed(self, column: &str) -> Result<Self, ConversionError> {
        match self {
            Self::Unsigned(value) => {
                Ok(Self::Integer(i64::try_from(value).map_err(|_| {
                    ConversionError::ValueOutOfRange(column.to_owned(), value)
                })?))
            }
            other => Ok(other),
        }
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<&&str> for Value {
    fn from(value: &&str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Self::Blob(value.to_owned())
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Self::Integer(value.into())
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Integer(value.into())
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Integer(if value { 1 } else { 0 })
    }
}

impl<T> From<Option<T>> for Value
where
    T: Into<Value>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}
