use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::pool::{self, Pool, ResourceManager};
use cdk_sql_common::value::Value;
use rusqlite::Connection;

/// The config need to create a new SQLite connection
#[derive(Clone, Debug)]
pub struct Config {
    path: Option<String>,
    password: Option<String>,
}

/// Sqlite connection manager
#[derive(Debug)]
pub struct SqliteConnectionManager;

impl ResourceManager for SqliteConnectionManager {
    type Config = Config;

    type Resource = Connection;

    type Error = rusqlite::Error;

    fn new_resource(
        config: &Self::Config,
        _stale: Arc<AtomicBool>,
        _timeout: Duration,
    ) -> Result<Self::Resource, pool::Error<Self::Error>> {
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

        Ok(conn)
    }
}

/// Create a configured rusqlite connection to a SQLite database.
/// For SQLCipher support, enable the "sqlcipher" feature and pass a password.
pub fn create_sqlite_pool(
    path: &str,
    password: Option<String>,
) -> Arc<Pool<SqliteConnectionManager>> {
    let (config, max_size) = if path.contains(":memory:") {
        (
            Config {
                path: None,
                password,
            },
            1,
        )
    } else {
        (
            Config {
                path: Some(path.to_owned()),
                password,
            },
            20,
        )
    };

    Pool::new(config, max_size, Duration::from_secs(10))
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
