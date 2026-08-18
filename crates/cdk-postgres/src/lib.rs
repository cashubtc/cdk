//! CDK Postgres

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, GenericTransactionHandler};
use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::pool::{DatabaseConfig, DatabasePool};
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::{SQLMintDatabase, SQLWalletDatabase};
use db::{pg_batch, pg_execute, pg_fetch_all, pg_fetch_one, pg_pluck};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use tokio::sync::{Mutex, Notify};
use tokio::time::timeout;
use tokio_postgres::{connect, Client, Error as PgError, NoTls};

mod db;
mod value;

#[derive(Debug)]
/// Postgres connection pool
pub struct PgConnectionPool;

#[derive(Clone)]
/// SSL Mode
pub enum SslMode {
    /// No TLS
    NoTls(NoTls),
    /// Native TLS
    NativeTls(postgres_native_tls::MakeTlsConnector),
}
const SSLMODE_VERIFY_FULL: &str = "sslmode=verify-full";
const SSLMODE_VERIFY_CA: &str = "sslmode=verify-ca";
const SSLMODE_PREFER: &str = "sslmode=prefer";
const SSLMODE_ALLOW: &str = "sslmode=allow";
const SSLMODE_REQUIRE: &str = "sslmode=require";

impl Default for SslMode {
    fn default() -> Self {
        SslMode::NoTls(NoTls {})
    }
}

impl fmt::Debug for SslMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let debug_text = match self {
            Self::NoTls(_) => "NoTls",
            Self::NativeTls(_) => "NativeTls",
        };

        write!(f, "SslMode::{debug_text}")
    }
}

/// Postgres configuration
#[derive(Clone)]
pub struct PgConfig {
    url: String,
    schema: Option<String>,
    tls: SslMode,
    max_connections: usize,
    connection_timeout: Duration,
}

impl fmt::Debug for PgConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgConfig")
            .field("url", &"[redacted]")
            .field("schema", &self.schema)
            .field("tls", &self.tls)
            .field("max_connections", &self.max_connections)
            .field("connection_timeout", &self.connection_timeout)
            .finish()
    }
}

impl DatabaseConfig for PgConfig {
    fn default_timeout(&self) -> Duration {
        self.connection_timeout
    }

    fn max_size(&self) -> usize {
        self.max_connections
    }
}

/// Default maximum number of connections in the pool
const DEFAULT_MAX_CONNECTIONS: usize = 20;

/// Default connection timeout in seconds
const DEFAULT_CONNECTION_TIMEOUT_SECS: u64 = 10;

/// Build a TLS connector with the given certificate/hostname validation settings.
fn build_tls(accept_invalid_certs: bool, accept_invalid_hostnames: bool) -> SslMode {
    let mut builder = TlsConnector::builder();
    if accept_invalid_certs {
        builder.danger_accept_invalid_certs(true);
    }
    if accept_invalid_hostnames {
        builder.danger_accept_invalid_hostnames(true);
    }

    match builder.build() {
        Ok(connector) => {
            let make_tls_connector = MakeTlsConnector::new(connector);
            SslMode::NativeTls(make_tls_connector)
        }
        Err(_) => SslMode::NoTls(NoTls {}),
    }
}

/// Determine TLS mode from the `sslmode=` parameter in a connection URL.
fn ssl_mode_from_url(url: &str) -> SslMode {
    if url.contains(SSLMODE_VERIFY_FULL) {
        // Strict TLS: valid certs and hostnames required
        build_tls(false, false)
    } else if url.contains(SSLMODE_VERIFY_CA) {
        // Verify CA, but allow invalid hostnames
        build_tls(false, true)
    } else if url.contains(SSLMODE_PREFER)
        || url.contains(SSLMODE_ALLOW)
        || url.contains(SSLMODE_REQUIRE)
    {
        // Lenient TLS for preferred/allow/require: accept invalid certs and hostnames
        build_tls(true, true)
    } else {
        SslMode::NoTls(NoTls {})
    }
}

/// Resolve TLS mode from an explicit `tls_mode` string (from config/env), such
/// as `"disable"`, `"prefer"`, `"require"`, `"verify-ca"`, or `"verify-full"`.
///
/// If the value is `None`, falls back to parsing `sslmode=` from the URL.
fn ssl_mode_from_config(tls_mode: Option<&str>, url: &str) -> SslMode {
    match tls_mode {
        Some(mode) => match mode.to_lowercase().as_str() {
            "verify-full" => build_tls(false, false),
            "verify-ca" => build_tls(false, true),
            "require" | "prefer" | "allow" => build_tls(true, true),
            // "disable" or any unrecognised value → no TLS
            _ => SslMode::NoTls(NoTls {}),
        },
        // No explicit tls_mode: fall back to URL-based detection
        None => ssl_mode_from_url(url),
    }
}

impl PgConfig {
    /// Create a new `PgConfig` with explicit TLS mode, pool size, and timeout.
    ///
    /// `tls_mode` accepts the same strings as the configuration file:
    /// `"disable"`, `"prefer"`, `"allow"`, `"require"`, `"verify-ca"`,
    /// `"verify-full"`.  When `None`, the TLS mode is inferred from
    /// `sslmode=` in the connection URL (matching the old behaviour).
    pub fn new(
        conn_str: &str,
        tls_mode: Option<&str>,
        max_connections: Option<usize>,
        connection_timeout_secs: Option<u64>,
    ) -> Self {
        let (schema, conn_str) = Self::strip_schema(conn_str);
        let tls = ssl_mode_from_config(tls_mode, &conn_str);
        PgConfig {
            url: conn_str,
            schema,
            tls,
            max_connections: max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
            connection_timeout: Duration::from_secs(
                connection_timeout_secs.unwrap_or(DEFAULT_CONNECTION_TIMEOUT_SECS),
            ),
        }
    }

    /// strip schema from the connection string
    fn strip_schema(input: &str) -> (Option<String>, String) {
        let mut schema: Option<String> = None;

        // Split by whitespace
        let mut parts = Vec::new();
        for token in input.split_whitespace() {
            if let Some(rest) = token.strip_prefix("schema=") {
                schema = Some(rest.to_string());
            } else {
                parts.push(token);
            }
        }

        let cleaned = parts.join(" ");
        (schema, cleaned)
    }
}

impl From<&str> for PgConfig {
    fn from(conn_str: &str) -> Self {
        let (schema, conn_str) = Self::strip_schema(conn_str);
        let tls = ssl_mode_from_url(&conn_str);

        PgConfig {
            url: conn_str,
            schema,
            tls,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            connection_timeout: Duration::from_secs(DEFAULT_CONNECTION_TIMEOUT_SECS),
        }
    }
}

impl DatabasePool for PgConnectionPool {
    type Config = PgConfig;

    type Connection = PostgresConnection;

    type Error = PgError;

    fn new_resource(
        config: &Self::Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Connection, cdk_sql_common::pool::Error<Self::Error>> {
        Ok(PostgresConnection::new(config.to_owned(), timeout, stale))
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
    pub fn new(config: PgConfig, timeout: Duration, stale: Arc<AtomicBool>) -> Self {
        let failed = Arc::new(Mutex::new(None));
        let result = Arc::new(OnceLock::new());
        let notify = Arc::new(Notify::new());
        let error_clone = failed.clone();
        let result_clone = result.clone();
        let notify_clone = notify.clone();

        async fn select_schema(conn: &Client, schema: &str) -> Result<(), Error> {
            conn.batch_execute(&format!(
                r#"
                    CREATE SCHEMA IF NOT EXISTS "{schema}";
                    SET search_path TO "{schema}"
                    "#
            ))
            .await
            .map_err(|e| Error::Database(Box::new(e)))
        }

        tokio::spawn(async move {
            match config.tls {
                SslMode::NoTls(tls) => {
                    let (client, connection) = match connect(&config.url, tls).await {
                        Ok((client, connection)) => (client, connection),
                        Err(err) => {
                            *error_clone.lock().await =
                                Some(cdk_common::database::Error::Database(Box::new(err)));
                            stale.store(true, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    };

                    let stale_for_spawn = stale.clone();
                    tokio::spawn(async move {
                        let _ = connection.await;
                        stale_for_spawn.store(true, std::sync::atomic::Ordering::Release);
                    });

                    if let Some(schema) = config.schema.as_ref() {
                        if let Err(err) = select_schema(&client, schema).await {
                            *error_clone.lock().await = Some(err);
                            stale.store(true, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    }

                    let _ = result_clone.set(client);
                    notify_clone.notify_waiters();
                }
                SslMode::NativeTls(tls) => {
                    let (client, connection) = match connect(&config.url, tls).await {
                        Ok((client, connection)) => (client, connection),
                        Err(err) => {
                            *error_clone.lock().await =
                                Some(cdk_common::database::Error::Database(Box::new(err)));
                            stale.store(true, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    };

                    let stale_for_spawn = stale.clone();
                    tokio::spawn(async move {
                        let _ = connection.await;
                        stale_for_spawn.store(true, std::sync::atomic::Ordering::Release);
                    });

                    if let Some(schema) = config.schema.as_ref() {
                        if let Err(err) = select_schema(&client, schema).await {
                            *error_clone.lock().await = Some(err);
                            stale.store(true, std::sync::atomic::Ordering::Release);
                            notify_clone.notify_waiters();
                            return;
                        }
                    }

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

/// Mint DB implementation with PostgreSQL
pub type MintPgDatabase = SQLMintDatabase<PgConnectionPool>;

/// Mint Auth database with Postgres
pub type MintPgAuthDatabase = SQLMintAuthDatabase<PgConnectionPool>;

/// Wallet DB implementation with PostgreSQL
pub type WalletPgDatabase = SQLWalletDatabase<PgConnectionPool>;

/// Convenience free functions (cannot add inherent impls for a foreign type).
/// These mirror the Mint patterns and call through to the generic constructors.
pub async fn new_wallet_pg_database(conn_str: &str) -> Result<WalletPgDatabase, Error> {
    <SQLWalletDatabase<PgConnectionPool>>::new(conn_str).await
}

#[cfg(test)]
mod test {
    use cdk_common::{mint_db_test, wallet_db_test, QuoteId};

    use super::*;

    async fn provide_mint_db(test_id: String) -> MintPgDatabase {
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );

        let db_url = format!("{db_url} schema={test_id}");

        MintPgDatabase::new(db_url.as_str())
            .await
            .expect("database")
    }

    mint_db_test!(provide_mint_db);

    #[tokio::test]
    async fn mint_pool_accepts_single_connection_configuration() {
        use cdk_common::database::MintDatabase;

        let test_id = format!("test_single_connection_pool_{}", uuid::Uuid::new_v4());
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );
        let config = PgConfig::new(
            &format!("{db_url} schema={test_id}"),
            None,
            Some(1),
            Some(10),
        );

        let db = MintPgDatabase::new(config)
            .await
            .expect("single-connection mint pool should remain supported");
        let dispatch = MintDatabase::begin_dispatch_transaction(&db)
            .await
            .expect("dispatch transaction");
        dispatch.rollback().await.expect("dispatch rollback");
        let regular = MintDatabase::begin_transaction(&db)
            .await
            .expect("regular transaction");
        regular.rollback().await.expect("regular rollback");
    }

    #[tokio::test]
    async fn try_quote_lock_reports_contended_without_waiting() {
        use std::sync::Arc;

        use cdk_common::database::mint::QuoteLockAttempt;
        use cdk_common::database::MintDatabase;

        let test_id = format!("test_try_quote_lock_{}", uuid::Uuid::new_v4());
        let db = Arc::new(provide_mint_db(test_id).await);
        let quote_id = QuoteId::new();

        let mut holder = MintDatabase::begin_transaction(&*db).await.expect("tx");
        assert!(holder
            .lock_quotes(std::slice::from_ref(&quote_id))
            .await
            .expect("lock"));

        let mut waiter = MintDatabase::begin_transaction(&*db).await.expect("tx");
        assert_eq!(
            waiter
                .try_lock_quotes(std::slice::from_ref(&quote_id))
                .await
                .expect("try lock"),
            QuoteLockAttempt::Contended
        );
        waiter.rollback().await.expect("rollback");

        holder.commit().await.expect("commit");

        let mut free = MintDatabase::begin_transaction(&*db).await.expect("tx");
        assert_eq!(
            free.try_lock_quotes(std::slice::from_ref(&quote_id))
                .await
                .expect("try lock"),
            QuoteLockAttempt::Acquired
        );
        free.rollback().await.expect("rollback");
    }

    #[tokio::test]
    async fn dispatch_transaction_preserves_regular_pool_capacity() {
        use std::time::Duration;

        use cdk_common::database::MintDatabase;

        let test_id = format!("test_dispatch_pool_{}", uuid::Uuid::new_v4());
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );
        let config = PgConfig::new(
            &format!("{db_url} schema={test_id}"),
            None,
            Some(2),
            Some(10),
        );
        let db = MintPgDatabase::new(config).await.expect("database");

        let dispatch = MintDatabase::begin_dispatch_transaction(&db)
            .await
            .expect("dispatch transaction");
        let regular =
            tokio::time::timeout(Duration::from_secs(5), MintDatabase::begin_transaction(&db))
                .await
                .expect("regular pool capacity must remain available")
                .expect("regular transaction");

        regular.rollback().await.expect("regular rollback");
        dispatch.rollback().await.expect("dispatch rollback");
    }

    #[tokio::test]
    async fn quote_lock_batch_excludes_concurrent_transaction() {
        use std::sync::Arc;
        use std::time::Duration;

        use cdk_common::database::MintDatabase;

        let test_id = format!("test_quote_lock_batch_{}", uuid::Uuid::new_v4());
        let db = Arc::new(provide_mint_db(test_id).await);
        let first = QuoteId::new();
        let second = QuoteId::new();

        let mut holder = MintDatabase::begin_transaction(&*db).await.expect("tx");
        holder
            .lock_quotes(&[first.clone(), second.clone()])
            .await
            .expect("lock");

        let waiter = tokio::spawn({
            let db = db.clone();
            async move {
                let mut tx = MintDatabase::begin_transaction(&*db).await.expect("tx");
                tx.lock_quotes(&[second, first]).await.expect("lock");
                tx.commit().await.expect("commit");
            }
        });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !waiter.is_finished(),
            "reversed quote batch did not wait for the holder"
        );

        holder.commit().await.expect("commit");
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("reversed quote batch remained blocked")
            .expect("waiter task");
    }

    #[tokio::test]
    async fn kvstore_compare_and_swap() {
        let test_id = format!("test_kvstore_compare_and_swap_{}", uuid::Uuid::new_v4());
        cdk_common::database::mint::test::kvstore_compare_and_swap(provide_mint_db(test_id).await)
            .await;
    }

    #[tokio::test]
    async fn concurrent_mint_quote_batches_use_consistent_lock_order() {
        let test_id = format!(
            "test_concurrent_mint_quote_batches_{}",
            uuid::Uuid::new_v4()
        );
        cdk_common::database::mint::test::concurrent_mint_quote_batches_use_consistent_lock_order(
            Arc::new(provide_mint_db(test_id).await),
        )
        .await;
    }

    #[tokio::test]
    async fn concurrent_multi_keyset_spends_use_consistent_lock_order() {
        let test_id = format!(
            "test_concurrent_multi_keyset_spends_{}",
            uuid::Uuid::new_v4()
        );
        cdk_common::database::mint::test::concurrent_multi_keyset_spends_use_consistent_lock_order(
            Arc::new(provide_mint_db(test_id).await),
        )
        .await;
    }

    async fn provide_wallet_db(test_id: String) -> WalletPgDatabase {
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );

        let db_url = format!("{db_url} schema={test_id}");

        WalletPgDatabase::new(db_url.as_str())
            .await
            .expect("database")
    }

    wallet_db_test!(provide_wallet_db);

    #[tokio::test]
    async fn failed_initial_connect_marks_connection_stale() {
        let stale = Arc::new(AtomicBool::new(false));
        let config = PgConfig::from("host=127.0.0.1 port=1 user=cdk dbname=cdk connect_timeout=1");
        let conn = PostgresConnection::new(config, Duration::from_secs(5), stale.clone());

        assert!(
            conn.inner().await.is_err(),
            "connect to refused port should fail"
        );
        tokio::task::yield_now().await;

        assert!(
            stale.load(std::sync::atomic::Ordering::SeqCst),
            "failed initial connect should mark the pooled connection stale"
        );
    }

    #[test]
    fn pgconfig_debug_does_not_leak_password() {
        let config = PgConfig::from("host=localhost user=u password=hunter2secret dbname=d");
        let rendered = format!("{config:?}");

        assert!(
            !rendered.contains("hunter2secret"),
            "PgConfig Debug leaked the DB password: {rendered}"
        );
    }
}
