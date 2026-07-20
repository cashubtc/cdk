//! Database-scoped daemon-instance and configuration-mutation guards.

#[cfg(feature = "sqlite")]
use std::fs::{File, OpenOptions};
#[cfg(feature = "sqlite")]
use std::future::pending;
#[cfg(feature = "sqlite")]
use std::io;
use std::path::Path;
#[cfg(feature = "sqlite")]
use std::path::PathBuf;

#[cfg(feature = "sqlite")]
use fs2::FileExt;
use thiserror::Error;

use crate::config::{Database, DatabaseEngine};

#[cfg(feature = "sqlite")]
const SQLITE_DAEMON_LOCK_FILE: &str = "cdk-mintd.lock";
#[cfg(feature = "sqlite")]
const SQLITE_CONFIGURATION_LOCK_FILE: &str = "cdk-mintd-config.lock";

pub(crate) const CONFIGURATION_BUSY_MESSAGE: &str =
    "configuration activation or another configuration command is in progress; retry";

#[derive(Debug, Clone, Copy)]
enum DatabaseLockPurpose {
    DaemonInstance,
    ConfigurationMutation,
}

impl DatabaseLockPurpose {
    #[cfg(feature = "sqlite")]
    fn sqlite_file(self) -> &'static str {
        match self {
            Self::DaemonInstance => SQLITE_DAEMON_LOCK_FILE,
            Self::ConfigurationMutation => SQLITE_CONFIGURATION_LOCK_FILE,
        }
    }
}

/// Failure to acquire exclusive access to a mintd configuration database.
#[derive(Debug, Error)]
pub(crate) enum DatabaseAccessError {
    /// Another daemon or direct configuration command owns the lock.
    #[error("the mintd configuration database is already in use")]
    Busy,

    /// The local SQLite lock file could not be opened.
    #[cfg(feature = "sqlite")]
    #[error("could not open database lock file {}: {source}", path.display())]
    OpenFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The local SQLite lock file could not be locked.
    #[cfg(feature = "sqlite")]
    #[error("could not lock database access through {}: {source}", path.display())]
    LockFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// PostgreSQL was selected without complete bootstrap settings.
    #[cfg(feature = "postgres")]
    #[error("PostgreSQL configuration is required to acquire the database lock")]
    MissingPostgresConfig,

    /// The PostgreSQL advisory lock session could not be established.
    #[cfg(feature = "postgres")]
    #[error(transparent)]
    Postgres(#[from] cdk_postgres::PgAdvisoryLockError),

    /// The selected database backend is not enabled in this build.
    #[cfg(any(not(feature = "sqlite"), not(feature = "postgres")))]
    #[error("{backend} support is not compiled into this cdk-mintd binary")]
    BackendUnavailable { backend: &'static str },
}

/// RAII guard covering the complete lifetime of database access.
#[derive(Debug)]
pub(crate) enum DatabaseAccessGuard {
    #[cfg(feature = "sqlite")]
    Sqlite { _file: File },
    #[cfg(feature = "postgres")]
    Postgres(cdk_postgres::PgAdvisoryLock),
}

impl DatabaseAccessGuard {
    /// Acquires the lifetime lock that prevents two daemons from running.
    pub(crate) async fn try_acquire_daemon_instance(
        work_dir: &Path,
        database: &Database,
    ) -> Result<Self, DatabaseAccessError> {
        Self::try_acquire(work_dir, database, DatabaseLockPurpose::DaemonInstance).await
    }

    /// Acquires the short-lived lock that serializes configuration mutation and activation.
    pub(crate) async fn try_acquire_configuration_mutation(
        work_dir: &Path,
        database: &Database,
    ) -> Result<Self, DatabaseAccessError> {
        Self::try_acquire(
            work_dir,
            database,
            DatabaseLockPurpose::ConfigurationMutation,
        )
        .await
    }

    async fn try_acquire(
        _work_dir: &Path,
        database: &Database,
        purpose: DatabaseLockPurpose,
    ) -> Result<Self, DatabaseAccessError> {
        match database.engine {
            DatabaseEngine::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    Self::try_acquire_sqlite(_work_dir, purpose)
                }

                #[cfg(not(feature = "sqlite"))]
                Err(DatabaseAccessError::BackendUnavailable { backend: "SQLite" })
            }
            DatabaseEngine::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let postgres = database
                        .postgres
                        .as_ref()
                        .ok_or(DatabaseAccessError::MissingPostgresConfig)?;
                    let config = cdk_postgres::PgConfig::new(
                        postgres.url.as_str(),
                        postgres.tls_mode.as_deref(),
                        postgres.max_connections,
                        postgres.connection_timeout_seconds,
                    );

                    let acquisition = match purpose {
                        DatabaseLockPurpose::DaemonInstance => {
                            cdk_postgres::PgAdvisoryLock::try_acquire(config).await
                        }
                        DatabaseLockPurpose::ConfigurationMutation => {
                            cdk_postgres::PgAdvisoryLock::try_acquire_configuration_mutation(config)
                                .await
                        }
                    };

                    match acquisition {
                        Ok(guard) => Ok(Self::Postgres(guard)),
                        Err(cdk_postgres::PgAdvisoryLockError::AlreadyHeld) => {
                            Err(DatabaseAccessError::Busy)
                        }
                        Err(error) => Err(DatabaseAccessError::Postgres(error)),
                    }
                }

                #[cfg(not(feature = "postgres"))]
                Err(DatabaseAccessError::BackendUnavailable {
                    backend: "PostgreSQL",
                })
            }
        }
    }

    #[cfg(feature = "sqlite")]
    fn try_acquire_sqlite(
        work_dir: &Path,
        purpose: DatabaseLockPurpose,
    ) -> Result<Self, DatabaseAccessError> {
        let path = work_dir.join(purpose.sqlite_file());
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| DatabaseAccessError::OpenFile {
                path: path.clone(),
                source,
            })?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self::Sqlite { _file: file }),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                Err(DatabaseAccessError::Busy)
            }
            Err(source) => Err(DatabaseAccessError::LockFile { path, source }),
        }
    }

    /// Returns an owned signal that resolves if a database-scoped lock is lost.
    pub(crate) fn loss_signal(&self) -> DatabaseLockLoss {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Sqlite { .. } => DatabaseLockLoss::Never,
            #[cfg(feature = "postgres")]
            Self::Postgres(guard) => DatabaseLockLoss::Postgres(guard.loss_signal()),
        }
    }
}

/// Owned notification used to stop work if an advisory-lock session dies.
#[derive(Debug)]
pub(crate) enum DatabaseLockLoss {
    #[cfg(feature = "sqlite")]
    Never,
    #[cfg(feature = "postgres")]
    Postgres(cdk_postgres::PgAdvisoryLockLossSignal),
}

impl DatabaseLockLoss {
    pub(crate) fn is_lost(&self) -> bool {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Never => false,
            #[cfg(feature = "postgres")]
            Self::Postgres(signal) => signal.is_lost(),
        }
    }

    pub(crate) async fn wait(self) {
        match self {
            #[cfg(feature = "sqlite")]
            Self::Never => pending::<()>().await,
            #[cfg(feature = "postgres")]
            Self::Postgres(signal) => signal.wait().await,
        }
    }
}

/// Immediately terminates the process after unexpected advisory-lock loss.
///
/// A graceful drain is unsafe here: PostgreSQL has already released the lock,
/// so another daemon can acquire it while old request handlers are still
/// running. Exiting without unwinding gives fail-stop behavior without creating
/// a core dump that could contain resolved configuration secrets, and forces
/// PostgreSQL to close or roll back this process's other sessions.
pub(crate) fn fail_stop_after_lock_loss(operation: &str) -> ! {
    tracing::error!(
        operation,
        "Exclusive PostgreSQL database lock was lost; exiting mintd immediately"
    );
    eprintln!(
        "fatal: exclusive PostgreSQL database lock was lost during {operation}; exiting immediately"
    );
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "sqlite")]
    use std::fs;

    #[cfg(feature = "sqlite")]
    use super::*;

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_locks_are_exclusive_independent_and_released_on_drop() {
        let work_dir = crate::test_utils::unique_temp_path("cdk_mintd_database_lock");
        fs::create_dir_all(&work_dir).expect("create temporary work directory");
        let database = Database::default();

        let first = DatabaseAccessGuard::try_acquire_daemon_instance(&work_dir, &database)
            .await
            .expect("first lock should succeed");
        let configuration =
            DatabaseAccessGuard::try_acquire_configuration_mutation(&work_dir, &database)
                .await
                .expect("configuration lock should be independent from the daemon lock");
        let second = DatabaseAccessGuard::try_acquire_daemon_instance(&work_dir, &database)
            .await
            .expect_err("second lock should be rejected");
        assert!(matches!(second, DatabaseAccessError::Busy));
        let second_configuration =
            DatabaseAccessGuard::try_acquire_configuration_mutation(&work_dir, &database)
                .await
                .expect_err("second configuration lock should be rejected");
        assert!(matches!(second_configuration, DatabaseAccessError::Busy));

        drop(first);
        DatabaseAccessGuard::try_acquire_daemon_instance(&work_dir, &database)
            .await
            .expect("daemon lock should be released when its guard is dropped");
        drop(configuration);
        DatabaseAccessGuard::try_acquire_configuration_mutation(&work_dir, &database)
            .await
            .expect("configuration lock should be released when its guard is dropped");

        fs::remove_dir_all(work_dir).expect("remove temporary work directory");
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_locks_are_scoped_to_the_work_directory() {
        let first_dir = crate::test_utils::unique_temp_path("cdk_mintd_database_lock_first");
        let second_dir = crate::test_utils::unique_temp_path("cdk_mintd_database_lock_second");
        fs::create_dir_all(&first_dir).expect("create first temporary work directory");
        fs::create_dir_all(&second_dir).expect("create second temporary work directory");
        let database = Database::default();

        let first = DatabaseAccessGuard::try_acquire_daemon_instance(&first_dir, &database)
            .await
            .expect("first work directory should lock");
        let second = DatabaseAccessGuard::try_acquire_daemon_instance(&second_dir, &database)
            .await
            .expect("second work directory should lock independently");

        drop(first);
        drop(second);
        fs::remove_dir_all(first_dir).expect("remove first temporary work directory");
        fs::remove_dir_all(second_dir).expect("remove second temporary work directory");
    }
}
