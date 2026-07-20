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
use tokio::sync::{watch, Mutex, Notify};
use tokio::task::JoinHandle;
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

// This namespace participates in cross-version mutual exclusion. Changing it
// would let daemons built from different CDK versions acquire different locks.
const MINTD_DAEMON_LOCK_NAMESPACE: &[u8] = b"cdk-mintd/database-access-lock/v1";
const MINTD_CONFIGURATION_LOCK_NAMESPACE: &[u8] = b"cdk-mintd/configuration-mutation-lock/v1";
const FNV_1A_64_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_1A_64_PRIME: u64 = 0x00000100000001b3;

/// Errors returned while acquiring the mintd PostgreSQL advisory lock.
#[derive(Debug, thiserror::Error)]
pub enum PgAdvisoryLockError {
    /// Another daemon or direct configuration command already holds the lock.
    #[error("the mintd PostgreSQL advisory lock is already held")]
    AlreadyHeld,

    /// The configured search path does not resolve to a schema.
    #[error("the PostgreSQL connection has no current schema for the mintd advisory lock")]
    MissingSchema,

    /// The dedicated lock connection timed out.
    #[error("timed out after {timeout_seconds} seconds opening the mintd PostgreSQL lock session")]
    ConnectionTimeout {
        /// Configured connection timeout in seconds.
        timeout_seconds: u64,
    },

    /// The lock connection closed while the lock was being acquired.
    #[error("the mintd PostgreSQL lock session closed while acquiring the advisory lock")]
    ConnectionLost,

    /// PostgreSQL rejected the connection or advisory-lock operation.
    #[error("PostgreSQL mintd advisory-lock operation failed: {source}")]
    Postgres {
        /// PostgreSQL driver error.
        #[source]
        source: PgError,
    },
}

impl From<PgError> for PgAdvisoryLockError {
    fn from(source: PgError) -> Self {
        Self::Postgres { source }
    }
}

/// Cloneable notification that the dedicated advisory-lock connection closed.
///
/// A daemon should treat this notification as terminal. Once the connection is
/// lost, PostgreSQL has released the session-level advisory lock.
#[derive(Debug, Clone)]
pub struct PgAdvisoryLockLossSignal {
    receiver: watch::Receiver<bool>,
}

impl PgAdvisoryLockLossSignal {
    /// Returns whether the lock connection has already closed.
    pub fn is_lost(&self) -> bool {
        *self.receiver.borrow()
    }

    /// Waits until the lock connection closes or the lock guard is dropped.
    pub async fn wait(mut self) {
        if self.is_lost() {
            return;
        }

        loop {
            match self.receiver.changed().await {
                Ok(()) if *self.receiver.borrow() => return,
                Ok(()) => {}
                Err(_) => return,
            }
        }
    }
}

struct DedicatedLockSession {
    client: Option<Client>,
    connection_task: Option<JoinHandle<()>>,
    loss_sender: watch::Sender<bool>,
    loss_receiver: watch::Receiver<bool>,
}

impl DedicatedLockSession {
    fn client(&self) -> Result<&Client, PgAdvisoryLockError> {
        self.client
            .as_ref()
            .ok_or(PgAdvisoryLockError::ConnectionLost)
    }

    fn loss_signal(&self) -> PgAdvisoryLockLossSignal {
        PgAdvisoryLockLossSignal {
            receiver: self.loss_receiver.clone(),
        }
    }
}

impl Drop for DedicatedLockSession {
    fn drop(&mut self) {
        let _ = self.loss_sender.send(true);
        drop(self.client.take());
        if let Some(connection_task) = self.connection_task.take() {
            connection_task.abort();
        }
    }
}

/// RAII guard for mintd's PostgreSQL advisory-session lock.
///
/// The guard owns one dedicated PostgreSQL connection. Dropping it closes that
/// connection and releases the advisory lock. Use [`Self::loss_signal`] to stop
/// the daemon if the connection closes unexpectedly while the guard is alive.
pub struct PgAdvisoryLock {
    session: DedicatedLockSession,
}

impl fmt::Debug for PgAdvisoryLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgAdvisoryLock")
            .field("connection_lost", &self.session.loss_signal().is_lost())
            .finish_non_exhaustive()
    }
}

impl PgAdvisoryLock {
    /// Returns a cloneable signal for unexpected lock-connection loss.
    pub fn loss_signal(&self) -> PgAdvisoryLockLossSignal {
        self.session.loss_signal()
    }

    /// Acquires the database-and-schema-scoped mintd PostgreSQL advisory lock.
    ///
    /// The function opens one dedicated connection without creating or selecting a
    /// configured schema first, so callers can acquire the lock before running
    /// database migrations. A configured `schema=...` value identifies the scope;
    /// otherwise PostgreSQL's current schema is used. The PostgreSQL endpoint must
    /// preserve session affinity; transaction-pooled proxies are not supported.
    pub async fn try_acquire(config: PgConfig) -> Result<Self, PgAdvisoryLockError> {
        Self::try_acquire_with_namespace(config, MINTD_DAEMON_LOCK_NAMESPACE).await
    }

    /// Acquires mintd's short-lived configuration-mutation advisory lock.
    ///
    /// This lock is independent from the daemon-instance lock and serializes
    /// startup activation with direct configuration commands.
    pub async fn try_acquire_configuration_mutation(
        config: PgConfig,
    ) -> Result<Self, PgAdvisoryLockError> {
        Self::try_acquire_with_namespace(config, MINTD_CONFIGURATION_LOCK_NAMESPACE).await
    }

    async fn try_acquire_with_namespace(
        config: PgConfig,
        namespace: &[u8],
    ) -> Result<Self, PgAdvisoryLockError> {
        let configured_schema = config.schema.clone();
        let session = open_dedicated_lock_session(config).await?;
        let identity = session
            .client()?
            .query_one("SELECT current_database(), current_schema()", &[])
            .await?;
        let database: String = identity.try_get(0)?;
        let current_schema: Option<String> = identity.try_get(1)?;
        let schema = configured_schema
            .or(current_schema)
            .ok_or(PgAdvisoryLockError::MissingSchema)?;
        let key = mintd_advisory_lock_key(namespace, &database, &schema);
        let acquired: bool = session
            .client()?
            .query_one("SELECT pg_try_advisory_lock($1::bigint)", &[&key])
            .await?
            .try_get(0)?;

        if !acquired {
            return Err(PgAdvisoryLockError::AlreadyHeld);
        }
        if session.loss_signal().is_lost() {
            return Err(PgAdvisoryLockError::ConnectionLost);
        }

        Ok(Self { session })
    }
}

async fn open_dedicated_lock_session(
    config: PgConfig,
) -> Result<DedicatedLockSession, PgAdvisoryLockError> {
    let PgConfig {
        url,
        tls,
        connection_timeout,
        ..
    } = config;
    let timeout_seconds = connection_timeout.as_secs();
    let (loss_sender, loss_receiver) = watch::channel(false);

    let (client, connection_task) = match tls {
        SslMode::NoTls(tls) => {
            let (client, connection) = timeout(connection_timeout, connect(&url, tls))
                .await
                .map_err(|_| PgAdvisoryLockError::ConnectionTimeout { timeout_seconds })??;
            let loss_sender = loss_sender.clone();
            let connection_task = tokio::spawn(async move {
                if let Err(error) = connection.await {
                    tracing::warn!("Mintd PostgreSQL advisory-lock connection closed: {error}");
                }
                let _ = loss_sender.send(true);
            });
            (client, connection_task)
        }
        SslMode::NativeTls(tls) => {
            let (client, connection) = timeout(connection_timeout, connect(&url, tls))
                .await
                .map_err(|_| PgAdvisoryLockError::ConnectionTimeout { timeout_seconds })??;
            let loss_sender = loss_sender.clone();
            let connection_task = tokio::spawn(async move {
                if let Err(error) = connection.await {
                    tracing::warn!("Mintd PostgreSQL advisory-lock connection closed: {error}");
                }
                let _ = loss_sender.send(true);
            });
            (client, connection_task)
        }
    };

    Ok(DedicatedLockSession {
        client: Some(client),
        connection_task: Some(connection_task),
        loss_sender,
        loss_receiver,
    })
}

fn mintd_advisory_lock_key(namespace: &[u8], database: &str, schema: &str) -> i64 {
    let mut hash = FNV_1A_64_OFFSET_BASIS;
    for byte in namespace
        .iter()
        .chain([0].iter())
        .chain(database.as_bytes())
        .chain([0].iter())
        .chain(schema.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_1A_64_PRIME);
    }
    hash as i64
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
    use cdk_common::{mint_db_test, wallet_db_test};

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
    fn mintd_advisory_lock_key_is_stable_and_scoped() {
        let key = mintd_advisory_lock_key(MINTD_DAEMON_LOCK_NAMESPACE, "cdk_mint", "public");

        assert_eq!(key, 5_922_106_403_771_514_837);
        assert_eq!(
            key,
            mintd_advisory_lock_key(MINTD_DAEMON_LOCK_NAMESPACE, "cdk_mint", "public")
        );
        assert_ne!(
            key,
            mintd_advisory_lock_key(MINTD_DAEMON_LOCK_NAMESPACE, "other_mint", "public")
        );
        assert_ne!(
            key,
            mintd_advisory_lock_key(MINTD_DAEMON_LOCK_NAMESPACE, "cdk_mint", "tenant")
        );
        assert_ne!(
            key,
            mintd_advisory_lock_key(MINTD_CONFIGURATION_LOCK_NAMESPACE, "cdk_mint", "public",)
        );
    }

    #[tokio::test]
    async fn advisory_lock_loss_signal_is_cloneable_and_wakes() {
        let (sender, receiver) = watch::channel(false);
        let signal = PgAdvisoryLockLossSignal { receiver };
        let cloned = signal.clone();

        assert!(!signal.is_lost());
        sender
            .send(true)
            .expect("loss signal receiver should still be alive");
        cloned.wait().await;
        assert!(signal.is_lost());
    }

    #[tokio::test]
    async fn advisory_lock_unavailable_server_is_not_reported_as_contention() {
        let config = PgConfig::new(
            "host=127.0.0.1 port=1 user=cdk dbname=cdk connect_timeout=1",
            None,
            Some(1),
            Some(1),
        );

        let error = PgAdvisoryLock::try_acquire(config)
            .await
            .expect_err("an unavailable PostgreSQL server must fail");

        assert!(!matches!(error, PgAdvisoryLockError::AlreadyHeld));
    }

    fn live_advisory_lock_config(test_name: &str) -> PgConfig {
        let url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL"))
            .expect("set CDK_MINTD_DATABASE_URL or PG_DB_URL for ignored PostgreSQL lock tests");
        let schema = format!("cdk_mintd_lock_{test_name}_{}", std::process::id());
        PgConfig::new(
            format!("{url} schema={schema}").as_str(),
            None,
            Some(2),
            Some(10),
        )
    }

    async fn acquire_live_lock_eventually(config: PgConfig) -> PgAdvisoryLock {
        for _ in 0..50 {
            match PgAdvisoryLock::try_acquire(config.clone()).await {
                Ok(lock) => return lock,
                Err(PgAdvisoryLockError::AlreadyHeld) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("unexpected advisory-lock error: {error}"),
            }
        }
        panic!("advisory lock was not released within one second");
    }

    #[tokio::test]
    #[ignore = "requires a live PostgreSQL server"]
    async fn advisory_lock_contends_and_releases_for_same_database_and_schema() {
        let config = live_advisory_lock_config("contention");
        let first = PgAdvisoryLock::try_acquire(config.clone())
            .await
            .expect("first advisory lock should succeed");
        let configuration = PgAdvisoryLock::try_acquire_configuration_mutation(config.clone())
            .await
            .expect("configuration lock must be independent from the daemon lock");

        let error = PgAdvisoryLock::try_acquire(config.clone())
            .await
            .expect_err("second advisory lock should contend");
        assert!(matches!(error, PgAdvisoryLockError::AlreadyHeld));
        let error = PgAdvisoryLock::try_acquire_configuration_mutation(config.clone())
            .await
            .expect_err("second configuration lock should contend");
        assert!(matches!(error, PgAdvisoryLockError::AlreadyHeld));

        drop(first);
        acquire_live_lock_eventually(config).await;
        drop(configuration);
    }

    #[tokio::test]
    #[ignore = "requires a live PostgreSQL server with pg_terminate_backend permission"]
    async fn advisory_lock_reports_backend_termination_and_can_be_reacquired() {
        let config = live_advisory_lock_config("termination");
        let lock = PgAdvisoryLock::try_acquire(config.clone())
            .await
            .expect("advisory lock should succeed");
        let backend_pid: i32 = lock
            .session
            .client()
            .expect("lock client should exist")
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .expect("read lock backend PID")
            .try_get(0)
            .expect("decode lock backend PID");
        let loss_signal = lock.loss_signal();

        let killer = open_dedicated_lock_session(config.clone())
            .await
            .expect("open termination session");
        let terminated: bool = killer
            .client()
            .expect("termination client should exist")
            .query_one("SELECT pg_terminate_backend($1)", &[&backend_pid])
            .await
            .expect("terminate advisory-lock backend")
            .try_get(0)
            .expect("decode termination result");
        assert!(terminated);
        timeout(Duration::from_secs(5), loss_signal.wait())
            .await
            .expect("lock-loss signal should fire after backend termination");

        drop(lock);
        acquire_live_lock_eventually(config).await;
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
