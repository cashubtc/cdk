//! Rust to SQLite conversions.

use std::borrow::Cow;
use std::rc::Rc;
use std::sync::Arc;

use super::{Null, Value, ValueRef};
use crate::error::{Error, Result};

/// A value ready to bind: either borrowed from the Rust value or converted.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToSqlOutput<'a> {
    /// Borrowed from the source value.
    Borrowed(ValueRef<'a>),
    /// Converted into a new value.
    Owned(Value),
}

impl ToSqlOutput<'_> {
    pub(crate) fn as_value_ref(&self) -> ValueRef<'_> {
        match self {
            Self::Borrowed(value) => *value,
            Self::Owned(value) => ValueRef::from(value),
        }
    }
}

impl<'a, T> From<&'a T> for ToSqlOutput<'a>
where
    T: ?Sized,
    &'a T: Into<ValueRef<'a>>,
{
    fn from(value: &'a T) -> Self {
        Self::Borrowed(value.into())
    }
}

macro_rules! to_sql_output_owned {
    ($($t:ty),*) => {$(
        impl From<$t> for ToSqlOutput<'_> {
            fn from(value: $t) -> Self {
                Self::Owned(Value::from(value))
            }
        }
    )*};
}

to_sql_output_owned!(
    Null,
    bool,
    i8,
    i16,
    i32,
    i64,
    isize,
    u8,
    u16,
    u32,
    f32,
    f64,
    String,
    Vec<u8>
);

impl From<Value> for ToSqlOutput<'_> {
    fn from(value: Value) -> Self {
        Self::Owned(value)
    }
}

impl<'a> From<ValueRef<'a>> for ToSqlOutput<'a> {
    fn from(value: ValueRef<'a>) -> Self {
        Self::Borrowed(value)
    }
}

/// Converts a Rust value into a value SQLite can bind.
pub trait ToSql {
    /// Converts `self`.
    fn to_sql(&self) -> Result<ToSqlOutput<'_>>;
}

macro_rules! to_sql_copy {
    ($($t:ty),*) => {$(
        impl ToSql for $t {
            fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(*self))
            }
        }
    )*};
}

to_sql_copy!(Null, bool, i8, i16, i32, i64, isize, u8, u16, u32, f32, f64);

macro_rules! to_sql_checked {
    ($($t:ty),*) => {$(
        impl ToSql for $t {
            fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
                i64::try_from(*self)
                    .map(|value| ToSqlOutput::Owned(Value::Integer(value)))
                    .map_err(|error| Error::ToSqlConversionFailure(Box::new(error)))
            }
        }
    )*};
}

to_sql_checked!(u64, usize);

impl ToSql for str {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(self.as_bytes())))
    }
}

impl ToSql for String {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        self.as_str().to_sql()
    }
}

impl ToSql for [u8] {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Blob(self)))
    }
}

impl ToSql for Vec<u8> {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        self.as_slice().to_sql()
    }
}

impl<const N: usize> ToSql for [u8; N] {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        self.as_slice().to_sql()
    }
}

impl ToSql for Value {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::from(self)))
    }
}

impl ToSql for ValueRef<'_> {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(*self))
    }
}

impl ToSql for ToSqlOutput<'_> {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(self.as_value_ref()))
    }
}

impl<T> ToSql for Option<T>
where
    T: ToSql,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        match self {
            Some(value) => value.to_sql(),
            None => Ok(ToSqlOutput::Borrowed(ValueRef::Null)),
        }
    }
}

impl<T> ToSql for &T
where
    T: ToSql + ?Sized,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        (**self).to_sql()
    }
}

impl<T> ToSql for Box<T>
where
    T: ToSql + ?Sized,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        (**self).to_sql()
    }
}

impl<T> ToSql for Rc<T>
where
    T: ToSql + ?Sized,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        (**self).to_sql()
    }
}

impl<T> ToSql for Arc<T>
where
    T: ToSql + ?Sized,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        (**self).to_sql()
    }
}

impl<T> ToSql for Cow<'_, T>
where
    T: ToSql + ToOwned + ?Sized,
{
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        (**self).to_sql()
    }
}
