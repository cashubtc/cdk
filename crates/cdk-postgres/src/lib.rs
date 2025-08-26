use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, GenericTransactionHandler};
use cdk_sql_common::mint::ldk::SQLLdkDatabase;
use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::pool::{DatabaseConfig, DatabasePool};
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::{SQLMintDatabase, SQLWalletDatabase};
use db::{pg_batch, pg_execute, pg_fetch_all, pg_fetch_one, pg_pluck};
use tokio::sync::{Mutex, Notify};
use tokio::time::timeout;
use tokio_postgres::{connect, Client, Error as PgError, NoTls};

mod db;
mod value;

#[derive(Debug)]
pub struct PgConnectionPool;

#[derive(Clone)]
pub enum SslMode {
    NoTls(NoTls),
    NativeTls(postgres_native_tls::MakeTlsConnector),
}

impl Default for SslMode {
    fn default() -> Self {
        SslMode::NoTls(NoTls {})
    }
}

impl Debug for SslMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let debug_text = match self {
            Self::NoTls(_) => "NoTls",
            Self::NativeTls(_) => "NativeTls",
        };

        write!(f, "SslMode::{debug_text}")
    }
}

/// Postgres configuration
#[derive(Clone, Debug)]
pub struct PgConfig {
    url: String,
    tls: SslMode,
}

impl DatabaseConfig for PgConfig {
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn max_size(&self) -> usize {
        20
    }
}

impl From<&str> for PgConfig {
    fn from(value: &str) -> Self {
        PgConfig {
            url: value.to_owned(),
            tls: Default::default(),
        }
    }
}

impl DatabasePool for PgConnectionPool {
    type Config = PgConfig;

    type Connection = PostgresConnection;

    type Error = PgError;

    fn new_resource(
        config: &Self::Config,
        still_valid: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Connection, cdk_sql_common::pool::Error<Self::Error>> {
        Ok(PostgresConnection::new(
            config.to_owned(),
            timeout,
            still_valid,
        ))
    }
}

/// A postgres connection
#[derive(Debug)]
pub struct PostgresConnection {
    timeout: Duration,
    error: Arc<Mutex<Option<cdk_common::database::Error>>>,
    result: Arc<OnceLock<Client>>,
    notify: Arc<Notify>,
}

impl PostgresConnection {
    /// Creates a new instance
    pub fn new(config: PgConfig, timeout: Duration, still_valid: Arc<AtomicBool>) -> Self {
        let failed = Arc::new(Mutex::new(None));
        let result = Arc::new(OnceLock::new());
        let notify = Arc::new(Notify::new());
        let error_clone = failed.clone();
        let result_clone = result.clone();
        let notify_clone = notify.clone();

        tokio::spawn(async move {
            match config.tls {
                SslMode::NoTls(tls) => {
                    let (client, connection) = match connect(&config.url, tls).await {
                        Ok((client, connection)) => (client, connection),
                        Err(err) => {
                            *error_clone.lock().await =
                                Some(cdk_common::database::Error::Database(Box::new(err)));
                            still_valid.store(false, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    };

                    tokio::spawn(async move {
                        let _ = connection.await;
                        still_valid.store(false, std::sync::atomic::Ordering::Release);
                    });

                    let _ = result_clone.set(client);
                    notify_clone.notify_waiters();
                }
                SslMode::NativeTls(tls) => {
                    let (client, connection) = match connect(&config.url, tls).await {
                        Ok((client, connection)) => (client, connection),
                        Err(err) => {
                            *error_clone.lock().await =
                                Some(cdk_common::database::Error::Database(Box::new(err)));
                            still_valid.store(false, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    };

                    tokio::spawn(async move {
                        let _ = connection.await;
                        still_valid.store(false, std::sync::atomic::Ordering::Release);
                    });

                    let _ = result_clone.set(client);
                    notify_clone.notify_waiters();
                }
            }
        });

        Self {
            error: failed,
            timeout,
            result,
            notify,
        }
    }

    /// Gets the wrapped instance or the connection error. The connection is returned as reference,
    /// and the actual error is returned once, next times a generic error would be returned
    async fn inner(&self) -> Result<&Client, cdk_common::database::Error> {
        if let Some(client) = self.result.get() {
            return Ok(client);
        }

        if let Some(error) = self.error.lock().await.take() {
            return Err(error);
        }

        if timeout(self.timeout, self.notify.notified()).await.is_err() {
            return Err(cdk_common::database::Error::Internal("Timeout".to_owned()));
        }

        // Check result again
        if let Some(client) = self.result.get() {
            Ok(client)
        } else if let Some(error) = self.error.lock().await.take() {
            Err(error)
        } else {
            Err(cdk_common::database::Error::Internal(
                "Failed connection".to_owned(),
            ))
        }
    }
}

#[async_trait::async_trait]
impl DatabaseConnector for PostgresConnection {
    type Transaction = GenericTransactionHandler<Self>;
}

#[async_trait::async_trait]
impl DatabaseExecutor for PostgresConnection {
    fn name() -> &'static str {
        "postgres"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        pg_execute(self.inner().await?, statement).await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        pg_fetch_one(self.inner().await?, statement).await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        pg_fetch_all(self.inner().await?, statement).await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        pg_pluck(self.inner().await?, statement).await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        pg_batch(self.inner().await?, statement).await
    }
}
pub type LdkPgDatabase = SQLLdkDatabase<PgConnectionPool>;

/// Mint DB implementation with PostgreSQL
pub type MintPgDatabase = SQLMintDatabase<PgConnectionPool>;

/// Mint Auth database with Postgres
#[cfg(feature = "auth")]
pub type MintPgAuthDatabase = SQLMintAuthDatabase<PgConnectionPool>;

/// Mint DB implementation with PostgresSQL
pub type WalletPgDatabase = SQLWalletDatabase<PgConnectionPool>;

#[cfg(test)]
mod test {
    use cdk_common::mint_db_test;
    use once_cell::sync::Lazy;
    use tokio::sync::Mutex;

    use super::*;

    static MIGRATION_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    async fn provide_db() -> MintPgDatabase {
        let m = MIGRATION_LOCK.lock().await;
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or("host=localhost user=test password=test dbname=testdb port=5433".to_owned());
        let db = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("database");
        drop(m);
        db
    }

    mint_db_test!(provide_db);
}
