use std::fmt::Debug;

use cdk_sql_common::value::Value;
use tokio_postgres::types::{self, FromSql, ToSql};

#[derive(Debug)]
pub enum PgValue<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a str),
    Blob(&'a [u8]),
}

/// A value or column type this driver cannot represent.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The integer does not fit the column it is being written to
    #[error("Value {value} does not fit in postgres type {ty}")]
    ValueOutOfRange {
        /// The value that was rejected
        value: i64,
        /// The column type it was being written to
        ty: types::Type,
    },

    /// The column type has no mapping onto a [`Value`]
    #[error("Unsupported postgres type {0}")]
    UnsupportedType(types::Type),
}

/// Encodes an integer into `ty`, refusing a value the column cannot hold.
///
/// This driver overrides `to_sql_checked`, so the range guard tokio-postgres
/// normally applies never runs. Narrowing without a check wrote a negative
/// number that the checked read path could not decode, which failed every read
/// of the table rather than only its own row.
fn integer_to_sql(
    value: i64,
    ty: &types::Type,
    out: &mut types::private::BytesMut,
) -> Result<types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
    let out_of_range = || Error::ValueOutOfRange {
        value,
        ty: ty.clone(),
    };

    match *ty {
        types::Type::BOOL => (value != 0).to_sql_checked(ty, out),
        types::Type::INT2 => i16::try_from(value)
            .map_err(|_| out_of_range())?
            .to_sql_checked(ty, out),
        types::Type::INT4 => i32::try_from(value)
            .map_err(|_| out_of_range())?
            .to_sql_checked(ty, out),
        _ => value.to_sql_checked(ty, out),
    }
}

impl<'a> From<&'a Value> for PgValue<'a> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::Blob(b) => PgValue::Blob(b),
            Value::Text(text) => PgValue::Text(text.as_str()),
            Value::Null => PgValue::Null,
            Value::Integer(i) => PgValue::Integer(*i),
            Value::Real(r) => PgValue::Real(*r),
        }
    }
}

impl<'a> From<PgValue<'a>> for Value {
    fn from(val: PgValue<'a>) -> Self {
        match val {
            PgValue::Blob(value) => Value::Blob(value.to_owned()),
            PgValue::Text(value) => Value::Text(value.to_owned()),
            PgValue::Null => Value::Null,
            PgValue::Integer(n) => Value::Integer(n),
            PgValue::Real(r) => Value::Real(r),
        }
    }
}

impl<'a> FromSql<'a> for PgValue<'a> {
    fn accepts(_ty: &types::Type) -> bool {
        true
    }

    fn from_sql(
        ty: &types::Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(match *ty {
            types::Type::VARCHAR | types::Type::TEXT | types::Type::BPCHAR | types::Type::NAME => {
                PgValue::Text(<&str as FromSql>::from_sql(ty, raw)?)
            }
            types::Type::BOOL => PgValue::Integer(if <bool as FromSql>::from_sql(ty, raw)? {
                1
            } else {
                0
            }),
            types::Type::INT2 => PgValue::Integer(<i16 as FromSql>::from_sql(ty, raw)?.into()),
            types::Type::INT4 => PgValue::Integer(<i32 as FromSql>::from_sql(ty, raw)?.into()),
            types::Type::INT8 => PgValue::Integer(<i64 as FromSql>::from_sql(ty, raw)?),
            types::Type::BIT_ARRAY | types::Type::BYTEA | types::Type::UNKNOWN => {
                PgValue::Blob(<&[u8] as FromSql>::from_sql(ty, raw)?)
            }
            _ => return Err(Error::UnsupportedType(ty.clone()).into()),
        })
    }

    fn from_sql_null(_ty: &types::Type) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PgValue::Null)
    }
}

impl ToSql for PgValue<'_> {
    fn to_sql(
        &self,
        ty: &types::Type,
        out: &mut types::private::BytesMut,
    ) -> Result<types::IsNull, Box<dyn std::error::Error + Sync + Send>>
    where
        Self: Sized,
    {
        match self {
            PgValue::Blob(blob) => (*blob).to_sql(ty, out),
            PgValue::Text(text) => (*text).to_sql(ty, out),
            PgValue::Null => Ok(types::IsNull::Yes),
            PgValue::Real(r) => r.to_sql(ty, out),
            PgValue::Integer(i) => integer_to_sql(*i, ty, out),
        }
    }

    fn accepts(_ty: &types::Type) -> bool
    where
        Self: Sized,
    {
        true
    }

    fn encode_format(&self, ty: &types::Type) -> types::Format {
        match self {
            PgValue::Blob(blob) => blob.encode_format(ty),
            PgValue::Text(text) => text.encode_format(ty),
            PgValue::Null => types::Format::Text,
            PgValue::Real(r) => r.encode_format(ty),
            PgValue::Integer(i) => i.encode_format(ty),
        }
    }

    fn to_sql_checked(
        &self,
        ty: &types::Type,
        out: &mut types::private::BytesMut,
    ) -> Result<types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self {
            PgValue::Blob(blob) => blob.to_sql_checked(ty, out),
            PgValue::Text(text) => text.to_sql_checked(ty, out),
            PgValue::Null => Ok(types::IsNull::Yes),
            PgValue::Real(r) => r.to_sql_checked(ty, out),
            PgValue::Integer(i) => integer_to_sql(*i, ty, out),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn encode(value: i64, ty: &types::Type) -> Result<(), ()> {
        let mut out = types::private::BytesMut::new();
        let unchecked = PgValue::Integer(value).to_sql(ty, &mut out).map_err(|_| ());
        let mut out = types::private::BytesMut::new();
        let checked = PgValue::Integer(value)
            .to_sql_checked(ty, &mut out)
            .map_err(|_| ());

        assert_eq!(unchecked.is_ok(), checked.is_ok());
        checked.map(|_| ())
    }

    #[test]
    fn int4_accepts_values_that_fit() {
        assert!(encode(i32::MAX.into(), &types::Type::INT4).is_ok());
        assert!(encode(i32::MIN.into(), &types::Type::INT4).is_ok());
    }

    #[test]
    fn int4_rejects_values_that_do_not_fit() {
        assert!(encode(i64::from(i32::MAX) + 1, &types::Type::INT4).is_err());
        assert!(encode(i64::from(i32::MIN) - 1, &types::Type::INT4).is_err());
    }

    #[test]
    fn int2_rejects_values_that_do_not_fit() {
        assert!(encode(i16::MAX.into(), &types::Type::INT2).is_ok());
        assert!(encode(i64::from(i16::MAX) + 1, &types::Type::INT2).is_err());
    }

    #[test]
    fn int8_accepts_the_whole_range() {
        assert!(encode(i64::MAX, &types::Type::INT8).is_ok());
        assert!(encode(i64::MIN, &types::Type::INT8).is_ok());
    }

    /// The int2 decoder used to be the single byte `"char"` decoder.
    #[test]
    fn int2_decodes_the_full_range() {
        let raw = i16::MAX.to_be_bytes();
        let value = PgValue::from_sql(&types::Type::INT2, &raw).expect("decodes");
        assert!(matches!(value, PgValue::Integer(n) if n == i64::from(i16::MAX)));
    }

    #[test]
    fn unsupported_type_is_an_error() {
        assert!(PgValue::from_sql(&types::Type::NUMERIC, &[0, 0]).is_err());
    }
}
