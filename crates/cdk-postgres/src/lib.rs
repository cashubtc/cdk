use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_base::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_base::mint::SQLMintAuthDatabase;
use cdk_sql_base::pool::{Pool, PooledResource, ResourceManager};
use cdk_sql_base::stmt::{Column, Statement};
use cdk_sql_base::{SQLMintDatabase, SQLWalletDatabase};
use db::{gn_pluck, pg_batch, pg_execute, pg_fetch_all, pg_fetch_one};
use tokio::runtime::Handle;
use tokio_postgres::{connect, Client, Error as PgError, NoTls};

mod db;
mod value;

#[derive(Debug)]
pub struct PgConnectionPool;

/// Runs an async future synchronously, using any existing Tokio runtime if available,
/// or creating a temporary runtime if not.
pub fn run_async<F: std::future::Future>(fut: F) -> F::Output {
    if Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }
}

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

#[derive(Debug)]
pub struct PgConfig {
    url: String,
    tls: SslMode,
}

impl ResourceManager for PgConnectionPool {
    type Config = PgConfig;

    type Resource = Client;

    type Error = PgError;

    fn new_resource(
        config: &Self::Config,
        still_valid: Arc<AtomicBool>,
    ) -> Result<Self::Resource, cdk_sql_base::pool::Error<Self::Error>> {
        Ok(match &config.tls {
            SslMode::NoTls(tls) => {
                let (client, connection) = run_async(connect(&config.url, tls.to_owned()))?;

                tokio::spawn(async move {
                    let _ = connection.await;
                    still_valid.store(false, std::sync::atomic::Ordering::Release);
                });

                client
            }
            SslMode::NativeTls(tls) => {
                let (client, connection) = run_async(connect(&config.url, tls.to_owned()))?;

                tokio::spawn(async move {
                    let _ = connection.await;
                    still_valid.store(false, std::sync::atomic::Ordering::Release);
                });

                client
            }
        })
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
    conn: PooledResource<PgConnectionPool>,
    done: bool,
    _phantom: PhantomData<&'a ()>,
}

impl<'a> Drop for CdkPostgresTx<'a> {
    fn drop(&mut self) {
        if !self.done {
            let _ = run_async(self.conn.batch_execute("ROLLBACK"));
        }
    }
}

impl<'a> Debug for CdkPostgresTx<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PgTx")
    }
}

#[async_trait::async_trait]
impl DatabaseConnector for CdkPostgres {
    type Transaction<'a> = CdkPostgresTx<'a>;

    async fn begin(&self) -> Result<Self::Transaction<'_>, Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        conn.batch_execute("BEGIN TRANSACTION")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        Ok(Self::Transaction {
            conn,
            done: false,
            _phantom: PhantomData,
        })
    }
}

#[async_trait::async_trait]
impl<'a> DatabaseTransaction<'a> for CdkPostgresTx<'a> {
    async fn commit(mut self) -> Result<(), Error> {
        self.conn
            .batch_execute("COMMIT")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        self.done = true;
        Ok(())
    }

    async fn rollback(mut self) -> Result<(), Error> {
        self.conn
            .batch_execute("ROLLBACK")
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        self.done = true;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<'a> DatabaseExecutor for CdkPostgresTx<'a> {
    fn name() -> &'static str {
        "postgres"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        pg_execute(&self.conn, statement).await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        pg_fetch_one(&self.conn, statement).await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        pg_fetch_all(&self.conn, statement).await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        gn_pluck(&self.conn, statement).await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        pg_batch(&self.conn, statement).await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for CdkPostgres {
    fn name() -> &'static str {
        "postgres"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        pg_execute(
            &self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            statement,
        )
        .await
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        pg_fetch_one(
            &self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            statement,
        )
        .await
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        pg_fetch_all(
            &self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            statement,
        )
        .await
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        gn_pluck(
            &self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
            statement,
        )
        .await
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        pg_batch(
            &self.pool.get().map_err(|e| Error::Database(Box::new(e)))?,
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
        let db_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set in the environment");
        let db = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("database");
        drop(m);
        db
    }

    mint_db_test!(provide_db);
}
