//! The primary database that mintd opens, parsed from its engine and the database password.

#[cfg(feature = "sqlite")]
use std::fmt;
#[cfg(feature = "sqlite")]
use std::path::PathBuf;

use anyhow::{bail, Result};

#[cfg(feature = "postgres")]
use crate::config::PostgresConfig;
use crate::config::{Database, DatabaseEngine};

/// The primary database selected for this run, with the credentials its engine reads.
///
/// SQLite carries the key for its database files. PostgreSQL carries its connection
/// settings and no password, because its credentials are in the connection URL.
#[derive(Debug, Clone)]
pub(crate) enum PrimaryDatabase {
    /// SQLite database files in the work directory.
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteKey),
    /// PostgreSQL database.
    #[cfg(feature = "postgres")]
    Postgres(PostgresConfig),
}

impl PrimaryDatabase {
    /// Parses the database settings and the database password into the database to open.
    ///
    /// A password is accepted only by the engine that reads it. No database is opened here,
    /// so a rejected password leaves no file or connection behind.
    pub(crate) fn select(database: &Database, password: Option<String>) -> Result<Self> {
        tracing::info!("Using database engine: {:?}", database.engine);
        match (&database.engine, password) {
            (DatabaseEngine::Sqlite, password) => Self::sqlite(password),
            (DatabaseEngine::Postgres, Some(_)) => bail!(
                "The PostgreSQL database engine does not use a database password; \
                 set its credentials in the connection URL"
            ),
            (DatabaseEngine::Postgres, None) => Self::postgres(database),
        }
    }

    #[cfg(feature = "sqlite")]
    fn sqlite(password: Option<String>) -> Result<Self> {
        SqliteKey::new(password).map(Self::Sqlite)
    }

    #[cfg(not(feature = "sqlite"))]
    fn sqlite(_password: Option<String>) -> Result<Self> {
        bail!("SQLite support not compiled in. Enable the 'sqlite' feature to use SQLite database.")
    }

    #[cfg(feature = "postgres")]
    fn postgres(database: &Database) -> Result<Self> {
        let config = database.postgres.clone().ok_or_else(|| {
            anyhow::anyhow!("PostgreSQL configuration is required when using PostgreSQL engine")
        })?;
        if config.url.is_empty() {
            bail!("PostgreSQL URL is required. Set it in config file [database.postgres] section or via CDK_MINTD_POSTGRES_URL/CDK_MINTD_DATABASE_URL environment variable");
        }
        Ok(Self::Postgres(config))
    }

    #[cfg(not(feature = "postgres"))]
    fn postgres(_database: &Database) -> Result<Self> {
        bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature to use PostgreSQL database.")
    }
}

/// The key that mintd opens its SQLite databases with.
///
/// A build with the `sqlcipher` feature always holds a SQLCipher key, and a build without
/// it never holds one. Each SQLite database that mintd opens is therefore encrypted exactly
/// when the build can encrypt it, and the main and auth databases share the one key.
#[cfg(feature = "sqlite")]
#[derive(Clone)]
pub(crate) struct SqliteKey {
    #[cfg(feature = "sqlcipher")]
    sqlcipher: String,
}

#[cfg(feature = "sqlite")]
impl SqliteKey {
    #[cfg(feature = "sqlcipher")]
    fn new(password: Option<String>) -> Result<Self> {
        match password {
            Some(sqlcipher) => Ok(Self { sqlcipher }),
            None => bail!("Password required when sqlcipher feature is enabled"),
        }
    }

    #[cfg(not(feature = "sqlcipher"))]
    fn new(password: Option<String>) -> Result<Self> {
        match password {
            None => Ok(Self {}),
            Some(_) => bail!(
                "A database password requires the sqlcipher feature; \
                 this build cannot encrypt the SQLite database"
            ),
        }
    }

    /// The database at `path` with this key, in the form that cdk-sqlite opens.
    #[cfg(feature = "sqlcipher")]
    pub(crate) fn database(&self, path: PathBuf) -> (PathBuf, String) {
        (path, self.sqlcipher.clone())
    }

    /// The database at `path` with this key, in the form that cdk-sqlite opens.
    #[cfg(not(feature = "sqlcipher"))]
    pub(crate) fn database(&self, path: PathBuf) -> PathBuf {
        path
    }
}

#[cfg(feature = "sqlite")]
impl fmt::Debug for SqliteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SqliteKey([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PostgresConfig;

    fn selection_error(database: &Database, password: Option<&str>) -> String {
        match PrimaryDatabase::select(database, password.map(str::to_owned)) {
            Ok(selection) => panic!("expected a rejected selection, got {selection:?}"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn postgres_engine_rejects_database_password() {
        let database = Database {
            engine: DatabaseEngine::Postgres,
            postgres: Some(PostgresConfig {
                url: "postgresql://cdk@127.0.0.1:1/cdk".to_string(),
                ..Default::default()
            }),
        };

        let error = selection_error(&database, Some("pass-value"));

        assert!(
            error.contains("does not use a database password"),
            "{error}"
        );
        assert!(!error.contains("pass-value"), "{error}");
    }

    #[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
    #[test]
    fn sqlite_without_sqlcipher_rejects_database_password() {
        let error = selection_error(&Database::default(), Some("pass-value"));

        assert!(error.contains("requires the sqlcipher feature"), "{error}");
        assert!(!error.contains("pass-value"), "{error}");
    }

    #[cfg(feature = "sqlcipher")]
    #[test]
    fn sqlcipher_build_requires_database_password() {
        let error = selection_error(&Database::default(), None);

        assert!(error.contains("Password required"), "{error}");
    }

    #[cfg(feature = "sqlcipher")]
    #[test]
    fn sqlcipher_selection_does_not_print_the_key() {
        let selection =
            PrimaryDatabase::select(&Database::default(), Some("pass-value".to_string()))
                .expect("sqlite uses the password");

        assert!(matches!(selection, PrimaryDatabase::Sqlite(_)));
        assert!(!format!("{selection:?}").contains("pass-value"));
    }
}
