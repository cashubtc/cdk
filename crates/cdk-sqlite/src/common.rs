use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::pool::{self, DatabasePool};
use cdk_sql_common::value::Value;
use rusqlite::Connection;
use tokio::sync::Mutex;

use crate::async_sqlite;

/// The config need to create a new SQLite connection
#[derive(Clone)]
pub struct Config {
    path: Option<String>,
    password: Option<String>,
    write_gate: Arc<Mutex<()>>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("path", &self.path)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl pool::DatabaseConfig for Config {
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn max_size(&self) -> usize {
        if self.path.is_none() {
            1
        } else {
            20
        }
    }
}

/// Sqlite connection manager
#[derive(Debug)]
pub struct SqliteConnectionManager;

impl DatabasePool for SqliteConnectionManager {
    type Config = Config;

    type Connection = async_sqlite::AsyncSqlite;

    type Error = rusqlite::Error;

    fn new_resource(
        config: &Self::Config,
        _stale: Arc<AtomicBool>,
        _timeout: Duration,
    ) -> Result<Self::Connection, pool::Error<Self::Error>> {
        let conn = if let Some(path) = config.path.as_ref() {
            // Check if parent directory exists before attempting to open database
            let path_buf = PathBuf::from(path);
            if let Some(parent) = path_buf.parent() {
                if !parent.to_str().unwrap_or_default().is_empty() && !parent.exists() {
                    return Err(pool::Error::Resource(rusqlite::Error::InvalidPath(
                        path_buf.clone(),
                    )));
                }
            }
            Connection::open(path)?
        } else {
            Connection::open_in_memory()?
        };

        if let Some(password) = config.password.as_ref() {
            conn.pragma_update(None, "key", password)?;
        }

        conn.execute_batch(
            r#"
            pragma busy_timeout = 10000;
            pragma journal_mode = WAL;
            pragma synchronous = FULL;
            pragma temp_store = memory;
            pragma mmap_size = 5242880;
            pragma cache = shared;
            "#,
        )?;

        conn.busy_timeout(Duration::from_secs(10))?;

        Ok(async_sqlite::AsyncSqlite::new(
            conn,
            Arc::clone(&config.write_gate),
        ))
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
                write_gate: Arc::new(Mutex::new(())),
            }
        } else {
            Config {
                path: Some(path.to_owned()),
                password: None,
                write_gate: Arc::new(Mutex::new(())),
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
                write_gate: Arc::new(Mutex::new(())),
            }
        } else {
            Config {
                path: Some(path.to_owned()),
                password: Some(pass.to_owned()),
                write_gate: Arc::new(Mutex::new(())),
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

/// Convert from rusqlite ValueRef to cdk_sql_common::value::Value.
#[inline(always)]
pub fn from_sqlite(v: rusqlite::types::ValueRef<'_>) -> Value {
    match v {
        rusqlite::types::ValueRef::Blob(blob) => Value::Blob(blob.to_vec()),
        rusqlite::types::ValueRef::Integer(i) => Value::Integer(i),
        rusqlite::types::ValueRef::Null => Value::Null,
        rusqlite::types::ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Value::Text(text.to_owned()),
            Err(_) => Value::Blob(bytes.to_vec()),
        },
        rusqlite::types::ValueRef::Real(r) => Value::Real(r),
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
