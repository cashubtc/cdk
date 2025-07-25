use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::pool::{Pool, PooledResource, ResourceManager};
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::{SQLMintDatabase, SQLWalletDatabase};
use db::{gn_pluck, pg_batch, pg_execute, pg_fetch_all, pg_fetch_one};
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

/// A simple wrapper for the async connect, this would trigger the `connect` in another tokio task
/// that would eventually resolve
#[derive(Debug)]
pub struct FutureConnect {
    timeout: Duration,
    error: Arc<Mutex<Option<cdk_common::database::Error>>>,
    result: Arc<OnceLock<Client>>,
    notify: Arc<Notify>,
}

impl FutureConnect {
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
    pub async fn client(&self) -> Result<&Client, cdk_common::database::Error> {
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

impl ResourceManager for PgConnectionPool {
    type Config = PgConfig;

    type Resource = FutureConnect;

    type Error = PgError;

    fn new_resource(
        config: &Self::Config,
        still_valid: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Resource, cdk_sql_common::pool::Error<Self::Error>> {
        Ok(FutureConnect::new(config.to_owned(), timeout, still_valid))
    }
}

#[derive(Debug)]
pub struct CdkPostgres {
    pool: Arc<Pool<PgConnectionPool>>,
}

impl From<&str> for CdkPostgres {
    fn from(value: &str) -> Self {
        let config = PgConfig {
            url: value.to_owned(),
            tls: Default::default(),
        };
        let pool = Pool::<PgConnectionPool>::new(config, 10, Duration::from_secs(10));
        CdkPostgres { pool }
    }
}

pub struct CdkPostgresTx<'a> {
    conn: Option<PooledResource<PgConnectionPool>>,
    done: bool,
    _phantom: PhantomData<&'a ()>,
}

impl Drop for CdkPostgresTx<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if !self.done {
                tokio::spawn(async move {
                    let _ = conn
                        .client()
                        .await
                        .expect("client")
                        .batch_execute("ROLLBACK")
                        .await;
                });
            }
        }
    }
}

impl Debug for CdkPostgresTx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PgTx")
    }
}

#[async_trait::async_trait]
impl DatabaseConnector for CdkPostgres {
    type Transaction<'a> = CdkPostgresTx<'a>;

    async fn begin(&self) -> Result<Self::Transaction<'_>, Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        conn.client()
            .await?
            .batch_execute("BEGIN TRANSACTION")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        Ok(Self::Transaction {
            conn: Some(conn),
            done: false,
            _phantom: PhantomData,
        })
    }
}

#[async_trait::async_trait]
impl<'a> DatabaseTransaction<'a> for CdkPostgresTx<'a> {
    async fn commit(mut self) -> Result<(), Error> {
        self.conn
            .as_ref()
            .ok_or(Error::Internal("Missing connection".to_owned()))?
            .client()
            .await?
            .batch_execute("COMMIT")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        self.done = true;
        Ok(())
    }

    async fn rollback(mut self) -> Result<(), Error> {
        self.conn
            .as_ref()
            .ok_or(Error::Internal("Missing connection".to_owned()))?
            .client()
            .await?
            .batch_execute("ROLLBACK")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        self.done = true;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for CdkPostgresTx<'_> {
    fn name() -> &'static str {
        "postgres"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        pg_execute(
            self.conn
                .as_ref()
                .ok_or(Error::Internal("Missing connection".to_owned()))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        pg_fetch_one(
            self.conn
                .as_ref()
                .ok_or(Error::Internal("Missing connection".to_owned()))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        pg_fetch_all(
            self.conn
                .as_ref()
                .ok_or(Error::Internal("Missing connection".to_owned()))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        gn_pluck(
            self.conn
                .as_ref()
                .ok_or(Error::Internal("Missing connection".to_owned()))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        pg_batch(
            self.conn
                .as_ref()
                .ok_or(Error::Internal("Missing connection".to_owned()))?
                .client()
                .await?,
            statement,
        )
        .await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for CdkPostgres {
    fn name() -> &'static str {
        "postgres"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        pg_execute(
            self.pool
                .get()
                .map_err(|e| Error::Database(Box::new(e)))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        pg_fetch_one(
            self.pool
                .get()
                .map_err(|e| Error::Database(Box::new(e)))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        pg_fetch_all(
            self.pool
                .get()
                .map_err(|e| Error::Database(Box::new(e)))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        gn_pluck(
            self.pool
                .get()
                .map_err(|e| Error::Database(Box::new(e)))?
                .client()
                .await?,
            statement,
        )
        .await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        pg_batch(
            self.pool
                .get()
                .map_err(|e| Error::Database(Box::new(e)))?
                .client()
                .await?,
            statement,
        )
        .await
    }
}

/// Mint DB implementation with PostgreSQL
pub type MintPgDatabase = SQLMintDatabase<CdkPostgres>;

/// Mint Auth database with Postgres
#[cfg(feature = "auth")]
pub type MintPgAuthDatabase = SQLMintAuthDatabase<CdkPostgres>;

/// Mint DB implementation with PostgresSQL
pub type WalletPgDatabase = SQLWalletDatabase<CdkPostgres>;

#[cfg(test)]
mod test {
    use cdk_common::mint_db_test;
    use once_cell::sync::Lazy;
    use tokio::sync::Mutex;

    use super::*;

    static MIGRATION_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    async fn provide_db() -> MintPgDatabase {
        let m = MIGRATION_LOCK.lock().await;
        let db_url = std::env::var("DATABASE_URL")
            .unwrap_or("host=localhost user=test password=test dbname=testdb port=5433".to_owned());
        let db = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("database");
        drop(m);
        db
    }

    mint_db_test!(provide_db);
}
