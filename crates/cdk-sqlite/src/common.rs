use std::fmt;
use std::path::PathBuf;

use cdk_sql_common::value::Value;

/// The config need to create a new SQLite connection
#[derive(Clone)]
pub struct Config {
    pub(crate) path: Option<String>,
    pub(crate) password: Option<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("path", &self.path)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl From<PathBuf> for Config {
    fn from(path: PathBuf) -> Self {
        path.to_str().unwrap_or_default().into()
    }
}

impl From<(PathBuf, String)> for Config {
    fn from((path, password): (PathBuf, String)) -> Self {
        (path.to_str().unwrap_or_default(), password.as_str()).into()
    }
}

impl From<&PathBuf> for Config {
    fn from(path: &PathBuf) -> Self {
        path.to_str().unwrap_or_default().into()
    }
}

impl From<&str> for Config {
    fn from(path: &str) -> Self {
        if path.contains(":memory:") {
            Config {
                path: None,
                password: None,
            }
        } else {
            Config {
                path: Some(path.to_owned()),
                password: None,
            }
        }
    }
}

impl From<(&str, &str)> for Config {
    fn from((path, pass): (&str, &str)) -> Self {
        if path.contains(":memory:") {
            Config {
                path: None,
                password: Some(pass.to_owned()),
            }
        } else {
            Config {
                path: Some(path.to_owned()),
                password: Some(pass.to_owned()),
            }
        }
    }
}

/// Convert cdk_sql_common::value::Value to rusqlite Value
#[inline(always)]
pub fn to_sqlite(v: Value) -> rusqlite::types::Value {
    match v {
        Value::Blob(blob) => rusqlite::types::Value::Blob(blob),
        Value::Integer(i) => rusqlite::types::Value::Integer(i),
        Value::Null => rusqlite::types::Value::Null,
        Value::Text(t) => rusqlite::types::Value::Text(t),
        Value::Real(r) => rusqlite::types::Value::Real(r),
    }
}

/// Convert from rusqlite Valute to cdk_sql_common::value::Value
#[inline(always)]
pub fn from_sqlite(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Blob(blob) => Value::Blob(blob),
        rusqlite::types::Value::Integer(i) => Value::Integer(i),
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Text(t) => Value::Text(t),
        rusqlite::types::Value::Real(r) => Value::Real(r),
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn config_debug_redacts_sqlcipher_password() {
        let secret = "sqlcipher-password-secret";
        let config = Config::from(("wallet.sqlite", secret));

        let debug = format!("{config:?}");

        assert!(debug.contains("wallet.sqlite"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(secret));
    }
}
