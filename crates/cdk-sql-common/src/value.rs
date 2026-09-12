//! Generic Rust value representation for data from the database
use cdk_common::database::ConversionError;
use cdk_common::{Amount, CurrencyUnit};

/// Renders an amount as the eight bytes SQLite stores.
///
/// SQLite has no unsigned 64 bit type, so an amount is stored as its big endian
/// bytes. Big endian is what makes SQLite's `memcmp` over equal length blobs the
/// same order as the numeric one, so no query needs a cast to sort or compare
/// one.
pub fn amount_to_blob(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

/// Reads an amount back, or `None` when the blob is not one.
pub fn amount_from_blob(bytes: &[u8]) -> Option<u64> {
    <[u8; 8]>::try_from(bytes).ok().map(u64::from_be_bytes)
}

/// Generic Value representation of data from the any database
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The value is a `NULL` value.
    Null,
    /// The value is a signed integer.
    Integer(i64),
    /// The value is an unsigned integer bound for a signed column.
    ///
    /// Conversion into [`Value`] has to be infallible, so the value is carried
    /// unchanged until the statement is rendered, where the placeholder name is
    /// known and an out of range value can be named in the error.
    Unsigned(u64),
    /// The value is an amount, which spans the whole `u64` range.
    ///
    /// Amount columns are the only ones wide enough to hold it, so the variant
    /// stays distinct from [`Value::Unsigned`] all the way to the driver, which
    /// is what decides how the column represents it.
    ///
    /// Bind an amount as an [`Amount`], never as a bare `u64`: that becomes
    /// [`Value::Unsigned`] and reaches SQLite as an integer. Writing one is
    /// caught by the check constraint on every amount column, but comparing one
    /// is not, because SQLite orders every integer below every blob rather than
    /// failing. `Value` cannot tell which column a `u64` is headed for, so this
    /// is the one edge the types do not close.
    Amount(u64),
    /// The value is a floating point number.
    Real(f64),
    /// The value is a text string.
    Text(String),
    /// The value is a blob of data
    Blob(Vec<u8>),
}

impl Value {
    /// Narrows an unsigned value onto the signed integer column it is bound to.
    ///
    /// Amounts are left alone: their columns hold the whole `u64` range.
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

impl From<Amount> for Value {
    fn from(value: Amount) -> Self {
        Self::Amount(value.to_u64())
    }
}

impl From<Amount<CurrencyUnit>> for Value {
    fn from(value: Amount<CurrencyUnit>) -> Self {
        Self::Amount(value.to_u64())
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
