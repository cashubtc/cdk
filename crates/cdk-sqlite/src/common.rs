use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_sql_common::pool::{self, DatabasePool};
use cdk_sql_common::value::Value;
use rusqlite::{ffi, Connection, OptionalExtension};

use crate::async_sqlite;
use crate::sqlcipher_key::SqlcipherKey;

/// Where the database lives: a file at a path, or a private in-memory database.
#[derive(Debug)]
enum Location {
    File(PathBuf),
    Memory {
        /// Set once the in-memory database has been opened. It lives only inside
        /// its single connection, so a second open means that connection was
        /// discarded.
        opened: AtomicBool,
    },
}

impl Location {
    /// Only the exact name `:memory:` selects memory mode; every other path is
    /// a file.
    fn from_path(path: PathBuf) -> Self {
        if path.as_os_str() == ":memory:" {
            Self::Memory {
                opened: AtomicBool::new(false),
            }
        } else {
            Self::File(path)
        }
    }
}

impl Clone for Location {
    fn clone(&self) -> Self {
        match self {
            Self::File(path) => Self::File(path.clone()),
            // A cloned config describes a new pool with its own database.
            Self::Memory { .. } => Self::Memory {
                opened: AtomicBool::new(false),
            },
        }
    }
}

/// The config need to create a new SQLite connection
pub struct Config {
    location: Location,
    password: Option<SqlcipherKey>,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            location: self.location.clone(),
            password: self.password.clone(),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("location", &self.location)
            .field("password", &self.password)
            .finish()
    }
}

impl pool::DatabaseConfig for Config {
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }

    fn max_size(&self) -> usize {
        match self.location {
            Location::Memory { .. } => 1,
            Location::File(_) => 20,
        }
    }
}

/// The name handed to SQLite for a file location.
///
/// SQLite reads a name starting with `file:` as a URI and exactly `:memory:` as
/// a private in-memory database. A path that starts that way names a file in
/// the current directory, so it gets a `./` prefix and SQLite opens that file.
fn sqlite_name(path: &Path) -> Cow<'_, Path> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.starts_with(b"file:") || bytes == b":memory:" {
        Cow::Owned(Path::new(".").join(path))
    } else {
        Cow::Borrowed(path)
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
        let conn = match &config.location {
            Location::Memory { opened } => {
                if opened.load(Ordering::SeqCst) {
                    // Handing out a fresh, empty database would silently drop
                    // every previous write, so make all later operations fail
                    // instead.
                    tracing::error!(
                        "In-memory SQLite connection was discarded; its database is lost"
                    );
                    return Err(pool::Error::Resource(rusqlite::Error::SqliteFailure(
                        ffi::Error::new(ffi::SQLITE_CANTOPEN),
                        Some(
                            "in-memory database was lost after its connection was discarded"
                                .to_owned(),
                        ),
                    )));
                }
                Connection::open_in_memory()?
            }
            Location::File(path) => {
                if path.as_os_str().is_empty() {
                    // SQLite would open a private temporary database for an
                    // empty name, and every pooled connection would get its own.
                    return Err(pool::Error::Resource(rusqlite::Error::InvalidPath(
                        path.clone(),
                    )));
                }

                let name = sqlite_name(path);

                // Check if parent directory exists before attempting to open database
                if let Some(parent) = name.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        return Err(pool::Error::Resource(rusqlite::Error::InvalidPath(
                            path.clone(),
                        )));
                    }
                }
                Connection::open(&name)?
            }
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

        if let Location::Memory { opened } = &config.location {
            opened.store(true, Ordering::SeqCst);
        }

        Ok(async_sqlite::AsyncSqlite::new(conn, stale))
    }
}

impl From<PathBuf> for Config {
    fn from(path: PathBuf) -> Self {
        Self {
            location: Location::from_path(path),
            password: None,
        }
    }
}

impl From<(PathBuf, SqlcipherKey)> for Config {
    fn from((path, password): (PathBuf, SqlcipherKey)) -> Self {
        let mut config = Self::from(path);
        config.password = Some(password);
        config
    }
}

impl From<&PathBuf> for Config {
    fn from(path: &PathBuf) -> Self {
        Self::from(path.clone())
    }
}

impl From<&str> for Config {
    fn from(path: &str) -> Self {
        Self::from(PathBuf::from(path))
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
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool};

    use super::{sqlite_name, Config, SqliteConnectionManager};

    fn open(
        config: &Config,
    ) -> Result<super::async_sqlite::AsyncSqlite, pool::Error<rusqlite::Error>> {
        SqliteConnectionManager::new_resource(
            config,
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(5),
        )
    }

    #[tokio::test]
    #[cfg(not(feature = "sqlcipher"))]
    async fn key_that_sqlite_does_not_apply_fails_to_open() {
        // Plain SQLite ignores the key pragma, so the key is never applied.
        let config = Config::from((":memory:", "key".parse().expect("non-empty key")));

        let result = open(&config);

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

    #[test]
    fn only_the_exact_memory_name_selects_memory_mode() {
        assert_eq!(Config::from(":memory:").max_size(), 1);
        assert_eq!(Config::from(PathBuf::from(":memory:")).max_size(), 1);
        assert_eq!(Config::from(&PathBuf::from(":memory:")).max_size(), 1);
        assert_eq!(Config::from("/data/wallet:memory:.sqlite").max_size(), 20);
        assert_eq!(Config::from("wallet.sqlite").max_size(), 20);
    }

    #[test]
    fn names_sqlite_would_reinterpret_get_a_directory_prefix() {
        assert_eq!(
            sqlite_name(Path::new("file:wallet.sqlite?mode=memory")),
            Path::new("./file:wallet.sqlite?mode=memory")
        );
        assert_eq!(
            sqlite_name(Path::new("file:wallet.sqlite")),
            Path::new("./file:wallet.sqlite")
        );
        assert_eq!(sqlite_name(Path::new(":memory:")), Path::new("./:memory:"));
    }

    #[test]
    fn ordinary_paths_reach_sqlite_unchanged() {
        for path in [
            "wallet.sqlite",
            "/data/wallet.sqlite",
            "./file:wallet.sqlite",
            "dir/file:x",
        ] {
            assert_eq!(sqlite_name(Path::new(path)), Path::new(path));
        }
    }

    #[tokio::test]
    async fn empty_path_is_rejected() {
        let result = open(&Config::from(""));

        assert!(matches!(
            result,
            Err(pool::Error::Resource(rusqlite::Error::InvalidPath(_)))
        ));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn non_utf8_path_opens_that_file() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cdk-sqlite-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join(OsStr::from_bytes(b"wallet-\xff.sqlite"));

        let conn = open(&Config::from(path.clone())).expect("open the named file");
        <SqliteConnectionManager as DatabasePool>::drop(conn);

        let exists = path.exists();
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
        assert!(exists, "the database must be the file that was named");
    }
}
