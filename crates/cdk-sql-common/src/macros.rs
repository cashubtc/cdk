//! Collection of macros to generate code to digest data from a generic SQL databasex

/// Unpacks a vector of Column, and consumes it, parsing into individual variables, checking the
/// vector is big enough.
#[macro_export]
macro_rules! unpack_into {
    (let ($($var:ident),+) = $array:expr) => {
        let ($($var),+) = {
            let mut vec = $array.to_vec();
            vec.reverse();
            let required = 0 $(+ {let _ = stringify!($var); 1})+;
            if vec.len() < required {
                 Err($crate::ConversionError::MissingColumn(required, vec.len()))?;
            }
            Ok::<_, cdk_common::database::Error>((
                $(
                    vec.pop().expect(&format!("Checked length already for {}", stringify!($var)))
                ),+
            ))?
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
            _ => Err($crate::ConversionError::InvalidType(
                "Number".to_owned(),
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
