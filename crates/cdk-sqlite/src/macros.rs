/// Unpacks a row<Column>, and consumes it, parsing into individual variables, checking the
/// Vec<Column> is big enough
#[macro_export]
macro_rules! unpack_into {
    (let ($($var:ident),+) = $array:expr) => {
        let ($($var),+) = {
            let mut vec = $array.to_vec();
            vec.reverse();
            let required = 0 $(+ {let _ = stringify!($var); 1})+;
            if vec.len() < required {
                return Err(Error::MissingColumn(required, vec.len()));
            }
            Ok::<_, Error>((
                $(
                    vec.pop().expect(&format!("Checked length already for {}", stringify!($var)))
                ),+
            ))?
        };
    };
}

/// Parses a SQLite column as a string or NULL
#[macro_export]
macro_rules! column_as_nullable_string {
    ($col:expr, $callback_str:expr, $callback_bytes:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(Some(text).and_then($callback_str)),
            $crate::stmt::Column::Blob(bytes) => Ok(Some(bytes).and_then($callback_bytes)),
            $crate::stmt::Column::Null => Ok(None),
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
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
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
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
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
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
                Error::InvalidConversion(stringify!($col).to_owned(), "Number".to_owned())
            })?)),
            $crate::stmt::Column::Integer(n) => Ok(Some(n.try_into().map_err(|_| {
                Error::InvalidConversion(stringify!($col).to_owned(), "Number".to_owned())
            })?)),
            $crate::stmt::Column::Null => Ok(None),
            other => Err(Error::InvalidType(
                "Number".to_owned(),
                other.data_type().to_string(),
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
                Error::InvalidConversion(stringify!($col).to_owned(), "Number".to_owned())
            }),
            $crate::stmt::Column::Integer(n) => n.try_into().map_err(|_| {
                Error::InvalidConversion(stringify!($col).to_owned(), "Number".to_owned())
            }),
            other => Err(Error::InvalidType(
                "Number".to_owned(),
                other.data_type().to_string(),
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
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
            )),
        })?
    };
}

/// Parses a SQLite column as a binary
#[macro_export]
macro_rules! column_as_binary {
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(text.as_bytes().to_vec()),
            $crate::stmt::Column::Blob(bytes) => Ok(bytes.to_owned()),
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
            )),
        })?
    };
}

/// Parses a SQLite column as a string
#[macro_export]
macro_rules! column_as_string {
    ($col:expr, $callback_str:expr, $callback_bytes:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => $callback_str(&text).map_err(Error::from),
            $crate::stmt::Column::Blob(bytes) => $callback_bytes(&bytes).map_err(Error::from),
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
            )),
        })?
    };
    ($col:expr, $callback:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => $callback(&text).map_err(Error::from),
            $crate::stmt::Column::Blob(bytes) => {
                $callback(&String::from_utf8_lossy(&bytes)).map_err(Error::from)
            }
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
            )),
        })?
    };
    ($col:expr) => {
        (match $col {
            $crate::stmt::Column::Text(text) => Ok(text.to_owned()),
            $crate::stmt::Column::Blob(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
            other => Err(Error::InvalidType(
                "String".to_owned(),
                other.data_type().to_string(),
            )),
        })?
    };
}
