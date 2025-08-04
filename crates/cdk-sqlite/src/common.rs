use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::pool::{self, DatabasePool};
use cdk_sql_common::value::Value;
use rusqlite::Connection;

use crate::async_sqlite;

/// The config need to create a new SQLite connection
#[derive(Clone, Debug)]
pub struct Config {
    path: Option<String>,
    password: Option<String>,
}

impl pool::DatabaseConfig for Config {
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn max_size(&self) -> usize {
        if self.password.is_none() {
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
            Connection::open(path)?
        } else {
            Connection::open_in_memory()?
        };

        if let Some(password) = config.password.as_ref() {
            conn.execute_batch(&format!("pragma key = '{password}';"))?;
        }

        conn.execute_batch(
            r#"
            pragma busy_timeout = 10000;
            pragma journal_mode = WAL;
            pragma synchronous = normal;
            pragma temp_store = memory;
            pragma mmap_size = 30000000000;
            pragma cache = shared;
            "#,
        )?;

        conn.busy_timeout(Duration::from_secs(10))?;

        Ok(async_sqlite::AsyncSqlite::new(conn))
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
