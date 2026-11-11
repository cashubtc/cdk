use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::pool::{self, DatabasePool};
use cdk_sql_common::value::Value;
use rusqlite::{ffi, Connection, OptionalExtension};

use crate::async_sqlite;
use crate::sqlcipher_key::SqlcipherKey;

/// The config need to create a new SQLite connection
pub struct Config {
    path: Option<String>,
    password: Option<SqlcipherKey>,
    /// Set once the in-memory database has been opened. It lives only inside its
    /// single connection, so a second open means that connection was discarded.
    in_memory_opened: AtomicBool,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            password: self.password.clone(),
            // A cloned config describes a new pool with its own database.
            in_memory_opened: AtomicBool::new(false),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("path", &self.path)
            .field("password", &self.password)
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

    fn drop(resource: Self::Connection) {
        resource.close();
    }

    fn new_resource(
        config: &Self::Config,
        stale: Arc<AtomicBool>,
        _timeout: Duration,
    ) -> Result<Self::Connection, pool::Error<Self::Error>> {
        let in_memory = config.path.is_none();
        if in_memory && config.in_memory_opened.load(Ordering::SeqCst) {
            // Handing out a fresh, empty database would silently drop every
            // previous write, so make all later operations fail instead.
            tracing::error!("In-memory SQLite connection was discarded; its database is lost");
            return Err(pool::Error::Resource(rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_CANTOPEN),
                Some("in-memory database was lost after its connection was discarded".to_owned()),
            )));
        }

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
            // SQLCipher confirms a key it applied with a single `ok` row.
            let accepted: Option<String> = conn
                .pragma_update_and_check(None, "key", password.as_str(), |row| row.get(0))
                .optional()?;
            let Some("ok") = accepted.as_deref() else {
                return Err(pool::Error::Resource(rusqlite::Error::SqliteFailure(
                    ffi::Error::new(ffi::SQLITE_ERROR),
                    Some("SQLCipher did not accept the database key".to_owned()),
                )));
            };
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

        if in_memory {
            config.in_memory_opened.store(true, Ordering::SeqCst);
        }

        Ok(async_sqlite::AsyncSqlite::new(conn, stale))
    }
}

impl From<PathBuf> for Config {
    fn from(path: PathBuf) -> Self {
        path.to_str().unwrap_or_default().into()
    }
}

impl From<(PathBuf, SqlcipherKey)> for Config {
    fn from((path, password): (PathBuf, SqlcipherKey)) -> Self {
        (path.to_str().unwrap_or_default(), password).into()
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
                in_memory_opened: Default::default(),
            }
        } else {
            Config {
                path: Some(path.to_owned()),
                password: None,
                in_memory_opened: Default::default(),
            }
        }
    }
}

impl From<(&str, SqlcipherKey)> for Config {
    fn from((path, password): (&str, SqlcipherKey)) -> Self {
        let mut config = Self::from(path);
        config.password = Some(password);
        config
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

    #[tokio::test]
    #[cfg(not(feature = "sqlcipher"))]
    async fn key_that_sqlite_does_not_apply_fails_to_open() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::time::Duration;

        use cdk_sql_common::pool::{self, DatabasePool};

        use super::SqliteConnectionManager;

        // Plain SQLite ignores the key pragma, so the key is never applied.
        let config = Config::from((":memory:", "key".parse().expect("non-empty key")));

        let result = SqliteConnectionManager::new_resource(
            &config,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        );

        match result {
            Err(pool::Error::Resource(rusqlite::Error::SqliteFailure(_, Some(message)))) => {
                assert!(
                    message.contains("did not accept the database key"),
                    "{message}"
                );
            }
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("a key that was not applied must not open the database"),
        }
    }

    #[test]
    fn config_debug_redacts_sqlcipher_password() {
        let secret = "sqlcipher-password-secret";
        let config = Config::from(("wallet.sqlite", secret.parse().expect("non-empty key")));

        let debug = format!("{config:?}");

        assert!(debug.contains("wallet.sqlite"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(secret));
    }
}
