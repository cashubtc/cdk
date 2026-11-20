//! Owned SQLite values.

use super::{Null, Type};

/// An owned SQLite value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// NULL
    Null,
    /// 64-bit signed integer
    Integer(i64),
    /// 64-bit IEEE float
    Real(f64),
    /// UTF-8 text
    Text(String),
    /// Binary blob
    Blob(Vec<u8>),
}

impl Value {
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
}

impl From<Null> for Value {
    fn from(_: Null) -> Self {
        Self::Null
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Integer(i64::from(value))
    }
}

macro_rules! from_integer {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(value: $t) -> Self {
                Self::Integer(i64::from(value))
            }
        }
    )*};
}

from_integer!(i8, i16, i32, i64, u8, u16, u32);

impl From<isize> for Value {
    fn from(value: isize) -> Self {
        // isize is at most 64 bits on every platform SQLite supports.
        Self::Integer(value as i64)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::Real(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

impl<T> From<Option<T>> for Value
where
    T: Into<Self>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::Null,
        }
    }
}
