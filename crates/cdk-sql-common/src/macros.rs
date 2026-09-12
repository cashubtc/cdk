//! Collection of macros to generate code to digest data from a generic SQL databasex

/// Unpacks a vector of Column, and consumes it, parsing into individual variables, checking the
/// vector is big enough.
#[macro_export]
macro_rules! unpack_into {
    (let ($($var:ident),+) = $array:expr) => {
        #[allow(unused_parens)]
        let ($($var),+) = {
            let mut vec = $array.to_vec();
            vec.reverse();
            let required = 0 $(+ {let _ = stringify!($var); 1})+;
            if vec.len() < required {
                 Err($crate::ConversionError::MissingColumn(required, vec.len()))?;
            }
            (
                $(
                    vec.pop().expect(&format!("Checked length already for {}", stringify!($var)))
                ),+
            )
        };
    };
}

/// Parses a SQL column as a string or NULL
#[macro_export]
macro_rules! column_as_nullable_string {
    ($col:expr, $callback_str:expr, $callback_bytes:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text).and_then($callback_str)),
            $crate::stmt::Column::Blob(bytes) => Ok(Some(bytes).and_then($callback_bytes)),
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
    ($col:expr, $callback_str:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text).and_then($callback_str)),
            $crate::stmt::Column::Blob(bytes) => {
                Ok(Some(String::from_utf8_lossy(&bytes)).and_then($callback_str))
            }
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text.to_owned())),
            $crate::stmt::Column::Blob(bytes) => {
                Ok(Some(String::from_utf8_lossy(&bytes).to_string()))
            }
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a column as a number or NULL
#[macro_export]
macro_rules! column_as_nullable_number {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text.parse().map_err(|_| {
                $crate::ConversionError::InvalidConversion(
                    stringify!($col).to_owned(),
                    "Number".to_owned(),
                )
            })?)),
            $crate::stmt::Column::Integer(n) => Ok(Some(n.try_into().map_err(|_| {
                $crate::ConversionError::InvalidConversion(
                    stringify!($col).to_owned(),
                    "Number".to_owned(),
                )
            })?)),
            $crate::stmt::Column::Amount(n) => Ok(Some(n.try_into().map_err(|_| {
                $crate::ConversionError::ValueOutOfRange(stringify!($col).to_owned(), n)
            })?)),
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "Number".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a column as a number
#[macro_export]
macro_rules! column_as_number {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => text.parse().map_err(|_| {
                $crate::ConversionError::InvalidConversion(
                    stringify!($col).to_owned(),
                    "Number".to_owned(),
                )
            }),
            $crate::stmt::Column::Integer(n) => n.try_into().map_err(|_| {
                $crate::ConversionError::InvalidConversion(
                    stringify!($col).to_owned(),
                    "Number".to_owned(),
                )
            }),
            $crate::stmt::Column::Amount(n) => n.try_into().map_err(|_| {
                $crate::ConversionError::ValueOutOfRange(stringify!($col).to_owned(), n)
            }),
            _ => Err($crate::ConversionError::InvalidType(
                "Number".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a column as an amount, which spans the whole `u64` range
///
/// The two backends represent one differently, so this is the only macro that
/// reads an amount column: postgres decodes `numeric(20, 0)` into
/// `Column::Amount`, SQLite hands back the eight bytes it stores. Any other
/// variant means the column is not one an amount was written to.
#[macro_export]
macro_rules! column_as_u64 {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Amount(n) => Ok(n),
            $crate::stmt::Column::Blob(bytes) => $crate::value::amount_from_blob(&bytes)
                .ok_or_else(|| $crate::ConversionError::InvalidAmount(stringify!($col).to_owned())),
            _ => Err($crate::ConversionError::InvalidType(
                "Amount".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a column as an amount or NULL, see [`column_as_u64`]
#[macro_export]
macro_rules! column_as_nullable_u64 {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Amount(n) => Ok(Some(n)),
            $crate::stmt::Column::Blob(bytes) => $crate::value::amount_from_blob(&bytes)
                .map(Some)
                .ok_or_else(|| $crate::ConversionError::InvalidAmount(stringify!($col).to_owned())),
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "Amount".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a column as a NULL or Binary
#[macro_export]
macro_rules! column_as_nullable_binary {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text.as_bytes().to_vec())),
            $crate::stmt::Column::Blob(bytes) => Ok(Some(bytes.to_owned())),
            $crate::stmt::Column::Null => Ok(None),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a SQL column as a binary
#[macro_export]
macro_rules! column_as_binary {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(text.as_bytes().to_vec()),
            $crate::stmt::Column::Blob(bytes) => Ok(bytes.to_owned()),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

/// Parses a SQL column as a string
#[macro_export]
macro_rules! column_as_string {
    ($col:expr, $callback_str:expr, $callback_bytes:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => {
                $callback_str(&text).map_err($crate::ConversionError::from)
            }
            $crate::stmt::Column::Blob(bytes) => {
                $callback_bytes(&bytes).map_err($crate::ConversionError::from)
            }
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
    ($col:expr, $callback:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => {
                $callback(&text).map_err($crate::ConversionError::from)
            }
            $crate::stmt::Column::Blob(bytes) => {
                $callback(&String::from_utf8_lossy(&bytes)).map_err($crate::ConversionError::from)
            }
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(text.to_owned()),
            $crate::stmt::Column::Blob(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
            _ => Err($crate::ConversionError::InvalidType(
                "String".to_owned(),
                stringify!($col).to_owned(),
            )),
        })?
    };
}

#[cfg(test)]
mod tests {
    use cdk_common::database::ConversionError;

    use crate::stmt::Column;
    use crate::value::amount_to_blob;

    fn read(column: Column) -> Result<u64, ConversionError> {
        Ok(column_as_u64!(column))
    }

    fn read_nullable(column: Column) -> Result<Option<u64>, ConversionError> {
        Ok(column_as_nullable_u64!(column))
    }

    #[test]
    fn reads_both_backends_across_the_whole_range() {
        for value in [0, 1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
            assert_eq!(
                read(Column::Blob(amount_to_blob(value))).expect("blob"),
                value
            );
            assert_eq!(read(Column::Amount(value)).expect("amount"), value);
            assert_eq!(
                read_nullable(Column::Blob(amount_to_blob(value))).expect("blob"),
                Some(value)
            );
            assert_eq!(
                read_nullable(Column::Amount(value)).expect("amount"),
                Some(value)
            );
        }

        assert_eq!(read_nullable(Column::Null).expect("null"), None);
    }

    /// A blob of the wrong width is a column an amount was never written to, so
    /// it has to fail rather than decode to some other number.
    #[test]
    fn refuses_a_blob_that_is_not_eight_bytes() {
        for width in [0, 4, 7, 9, 16] {
            assert!(matches!(
                read(Column::Blob(vec![0; width])),
                Err(ConversionError::InvalidAmount(_))
            ));
            assert!(matches!(
                read_nullable(Column::Blob(vec![0; width])),
                Err(ConversionError::InvalidAmount(_))
            ));
        }
    }

    /// Text is the representation amounts used to have and integer is the one
    /// they had before that, so either arriving means a table the migration
    /// missed. Reading one as a number would hide that.
    #[test]
    fn refuses_every_other_variant() {
        for column in [
            Column::Text("42".to_owned()),
            Column::Integer(42),
            Column::Unsigned(42),
            Column::Real(42.0),
        ] {
            assert!(matches!(
                read(column.clone()),
                Err(ConversionError::InvalidType(_, _))
            ));
            assert!(matches!(
                read_nullable(column),
                Err(ConversionError::InvalidType(_, _))
            ));
        }

        assert!(matches!(
            read(Column::Null),
            Err(ConversionError::InvalidType(_, _))
        ));
    }
}
