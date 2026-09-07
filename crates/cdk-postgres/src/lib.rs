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
    #[cfg(feature = "federation")]
    use std::collections::{BTreeMap, BTreeSet};

    use cdk_common::{mint_db_test, wallet_db_test, QuoteId};

    use super::*;

    fn test_db_url_with_schema(test_id: &str) -> String {
        let db_url = std::env::var("CDK_MINTD_DATABASE_URL")
            .or_else(|_| std::env::var("PG_DB_URL")) // Fallback for compatibility
            .unwrap_or(
                "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                    .to_owned(),
            );

        format!("{db_url} schema={test_id}")
    }

    #[cfg(feature = "federation")]
    fn unique_federation_test_id(test_name: &str) -> String {
        let prefix = test_name.chars().take(24).collect::<String>();
        format!("fed_{prefix}_{}", uuid::Uuid::new_v4().simple())
    }

    async fn provide_mint_db(test_id: String) -> MintPgDatabase {
        let db_url = test_db_url_with_schema(&test_id);

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
        let regular = MintDatabase::begin_transaction(&db)
            .await
            .expect("regular transaction");
        regular.rollback().await.expect("regular rollback");
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
        let db_url = test_db_url_with_schema(&test_id);

        WalletPgDatabase::new(db_url.as_str())
            .await
            .expect("database")
    }

    wallet_db_test!(provide_wallet_db);

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_config_persistence() {
        cdk_common::database::mint::test::federation_config_persistence(
            provide_mint_db(unique_federation_test_id("federation_config_persistence")).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_setup_persistence() {
        cdk_common::database::mint::test::federation_setup_persistence(
            provide_mint_db(unique_federation_test_id("federation_setup_persistence")).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_setup_transaction_rollbacks() {
        cdk_common::database::mint::test::federation_setup_transaction_rollbacks(
            provide_mint_db(unique_federation_test_id(
                "federation_setup_transaction_rollbacks",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_setup_concurrent_races() {
        cdk_common::database::mint::test::federation_setup_concurrent_races(
            provide_mint_db(unique_federation_test_id(
                "federation_setup_concurrent_races",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_setup_invitation_replacement() {
        cdk_common::database::mint::test::federation_setup_invitation_replacement(
            provide_mint_db(unique_federation_test_id(
                "federation_setup_invitation_replacement",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_persistence() {
        cdk_common::database::mint::test::federation_journal_persistence(
            provide_mint_db(unique_federation_test_id("federation_journal_persistence")).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_application_state_persistence() {
        cdk_common::database::mint::test::federation_application_state_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_application_state_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_finalized_item_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_finalized_item_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_aleph_bft_finalized_item_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_checkpoint_persistence() {
        cdk_common::database::mint::test::federation_checkpoint_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_checkpoint_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_backup_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_backup_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_aleph_bft_backup_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_active_session_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_active_session_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_aleph_bft_active_session_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_aleph_bft_pending_operation_persistence() {
        cdk_common::database::mint::test::federation_aleph_bft_pending_operation_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_aleph_bft_pending_operation_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_payment_observation_persistence() {
        cdk_common::database::mint::test::federation_payment_observation_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_payment_observation_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_mint_quote_state_projection() {
        cdk_common::database::mint::test::federation_mint_quote_state_projection(
            provide_mint_db(unique_federation_test_id(
                "federation_mint_quote_state_projection",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_materialized_projection_keyed_reads() {
        cdk_common::database::mint::test::federation_materialized_projection_keyed_reads(
            provide_mint_db(unique_federation_test_id(
                "federation_materialized_projection_keyed_reads",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation-bark")]
    #[tokio::test]
    async fn federation_bark_projection_persistence() {
        cdk_sql_common::federation_bark_test_support::federation_bark_projection_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_bark_projection_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation-bark")]
    #[tokio::test]
    async fn federation_bark_action_reads() {
        cdk_sql_common::federation_bark_test_support::federation_bark_action_reads(
            provide_mint_db(unique_federation_test_id("federation_bark_action_reads")).await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_signature_share_persistence() {
        cdk_common::database::mint::test::federation_signature_share_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_signature_share_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_setup_state_persistence() {
        cdk_common::database::mint::test::federation_dkg_setup_state_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_setup_state_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_participant_state_persistence() {
        cdk_common::database::mint::test::federation_dkg_participant_state_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_participant_state_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_ceremony_persistence() {
        cdk_common::database::mint::test::federation_dkg_ceremony_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_ceremony_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_member_keyset_share_persistence() {
        cdk_common::database::mint::test::federation_dkg_member_keyset_share_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_member_keyset_share_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_secret_share_contribution_persistence() {
        cdk_common::database::mint::test::federation_dkg_secret_share_contribution_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_secret_share_contribution_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_transcript_signature_persistence() {
        cdk_common::database::mint::test::federation_dkg_transcript_signature_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_transcript_signature_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_dkg_activation_persistence() {
        cdk_common::database::mint::test::federation_dkg_activation_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_dkg_activation_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_frost_dkg_persistence() {
        cdk_common::database::mint::test::federation_frost_dkg_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_frost_dkg_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_frost_wallet_config_persistence() {
        cdk_common::database::mint::test::federation_frost_wallet_config_persistence(
            provide_mint_db(unique_federation_test_id(
                "federation_frost_wallet_config_persistence",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_concurrent_writes() {
        cdk_common::database::mint::test::federation_journal_concurrent_writes(
            provide_mint_db(unique_federation_test_id(
                "federation_journal_concurrent_writes",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_journal_concurrent_metadata_updates() {
        cdk_common::database::mint::test::federation_journal_concurrent_metadata_updates(
            provide_mint_db(unique_federation_test_id(
                "federation_journal_concurrent_metadata_updates",
            ))
            .await,
        )
        .await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federation_migrations_apply_to_empty_postgres_and_keep_standalone_tables() {
        let test_id = unique_federation_test_id("migrate_empty");
        let db_url = test_db_url_with_schema(&test_id);
        let migrated = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("migrated postgres database");

        drop(migrated);

        let config = PgConfig::from(db_url.as_str());
        let schema = config.schema.clone().expect("test schema");
        let (client, connection) = connect(&config.url, NoTls)
            .await
            .expect("postgres connection");
        let connection_handle = tokio::spawn(async move {
            let _ = connection.await;
        });

        let table_rows = client
            .query(
                r#"
                SELECT table_name
                FROM information_schema.tables
                WHERE table_schema = $1
                "#,
                &[&schema],
            )
            .await
            .expect("postgres table rows");
        let table_names = table_rows
            .into_iter()
            .map(|row| row.get::<_, String>(0))
            .collect::<BTreeSet<_>>();

        for table in [
            "proof",
            "keyset",
            "mint_quote",
            "melt_quote",
            "blind_signature",
            "saga_state",
            "completed_operations",
            "federation_journal_entries",
            "federation_dkg_member_sequences",
            "federation_checkpoints",
            "federation_checkpoint_signatures",
            "federation_checkpoint_quorums",
            "federation_checkpoint_state",
            "federation_materialized_state_keys",
            "federation_aleph_bft_finalized_items",
            "federation_aleph_bft_backups",
            "federation_aleph_bft_pending_operations",
            "federation_aleph_bft_session_state",
            "federation_payment_observations",
            "federation_config",
            "federation_signature_shares",
            "federation_dkg_setup_messages",
            "federation_dkg_participant_states",
            "federation_dkg_member_keyset_shares",
            "federation_dkg_secret_share_contributions",
            "federation_dkg_transcript_signatures",
            "federation_dkg_activations",
            "federation_dkg_ceremonies",
            "federation_frost_dkg_records",
            "federation_frost_wallet_config",
            "federation_setup_sessions",
            "federation_setup_invitations",
            "federation_setup_enrollments",
            "federation_setup_proposals",
            "federation_setup_approval_fences",
            "federation_setup_approvals",
            "federation_setup_manifests",
            "federation_setup_activations",
            "federation_bark_actions",
            "federation_bark_vtxos",
            "federation_bark_derivation_allocations",
            "federation_bark_reservations",
            "federation_bark_transitions",
            "federation_bark_observations",
        ] {
            assert!(
                table_names.contains(table),
                "missing migrated table {table}"
            );
        }
        assert!(
            !table_names.contains("federation_aleph_bft_network_retries"),
            "AlephBFT gossip retries must remain runtime-only"
        );

        let index_rows = client
            .query(
                r#"
                SELECT indexname
                FROM pg_indexes
                WHERE schemaname = $1
                "#,
                &[&schema],
            )
            .await
            .expect("postgres index rows");
        let index_names = index_rows
            .into_iter()
            .map(|row| row.get::<_, String>(0))
            .collect::<BTreeSet<_>>();

        for index in [
            "idx_federation_journal_operation_id",
            "idx_federation_checkpoints_kind_sequence",
            "idx_federation_checkpoints_finalized_cursor",
            "idx_federation_checkpoint_signatures_checkpoint",
            "idx_federation_materialized_state_keys_operation",
            "idx_federation_aleph_bft_finalized_items_session_unit",
            "idx_federation_aleph_bft_finalized_items_operation_id",
            "idx_federation_aleph_bft_pending_operations_session",
            "idx_federation_payment_observations_member_state",
            "idx_federation_payment_observations_quote",
            "idx_federation_signature_shares_operation_output",
            "idx_federation_signature_shares_blinded_message",
            "idx_federation_dkg_setup_member",
            "idx_federation_dkg_participant_states_member",
            "idx_federation_dkg_member_keyset_shares_member",
            "idx_federation_dkg_secret_share_contributions_receiver",
            "idx_federation_dkg_transcript_signatures_member",
            "idx_federation_dkg_activations_member",
            "idx_federation_frost_dkg_records_kind",
            "idx_federation_frost_wallet_config_kind",
            "idx_federation_setup_invitations_member",
            "idx_federation_bark_actions_phase",
            "idx_federation_bark_vtxos_state",
            "idx_federation_bark_vtxos_reservation",
            "idx_federation_bark_allocations_action",
            "idx_federation_bark_reservations_action",
            "idx_federation_bark_reservations_operation",
            "idx_federation_bark_transitions_action",
            "idx_federation_bark_observations_action",
            "blind_signature_quote_id_index",
        ] {
            assert!(
                index_names.contains(index),
                "missing migrated index {index}"
            );
        }

        let journal_column_rows = client
            .query(
                r#"
                SELECT column_name, is_nullable
                FROM information_schema.columns
                WHERE table_schema = $1
                  AND table_name = 'federation_journal_entries'
                "#,
                &[&schema],
            )
            .await
            .expect("postgres journal column rows");
        let journal_columns = journal_column_rows
            .into_iter()
            .map(|row| {
                let name = row.get::<_, String>(0);
                let required = row.get::<_, String>(1) == "NO";
                (name, required)
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(journal_columns.get("conflict_keys_json"), Some(&true));
        assert!(!journal_columns.contains_key("operation_outcome_hash"));
        assert!(!journal_columns.contains_key("materialized_state_digest"));
        for column in [
            "consensus_evidence_json",
            "rejection_json",
            "application_digest",
        ] {
            assert_eq!(journal_columns.get(column), Some(&false));
        }

        let rejection_on_applied = client
            .execute(
                r#"
                INSERT INTO federation_journal_entries
                (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, rejection_json, status)
                VALUES (0, 'invalid-rejection', '{}', '[]', 'previous', 'current', '{}', 'applied')
                "#,
                &[],
            )
            .await;
        assert!(rejection_on_applied.is_err());

        let unknown_status = client
            .execute(
                r#"
                INSERT INTO federation_journal_entries
                (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, status)
                VALUES (1, 'invalid-status', '{}', '[]', 'previous', 'current', 'unknown')
                "#,
                &[],
            )
            .await;
        assert!(unknown_status.is_err());

        let saga_column_count = client
            .query_one(
                r#"
                SELECT COUNT(*)
                FROM information_schema.columns
                WHERE table_schema = $1
                  AND table_name = 'saga_state'
                  AND column_name = 'federation_operation_id'
                "#,
                &[&schema],
            )
            .await
            .expect("postgres saga federation column")
            .get::<_, i64>(0);
        assert_eq!(saga_column_count, 1);

        let federation_migration_query = format!(
            r#"
            SELECT name
            FROM "{schema}".migrations
            WHERE name LIKE '%federation%'
            ORDER BY name
            "#
        );
        let federation_migrations = client
            .query(&federation_migration_query, &[])
            .await
            .expect("postgres federation migration rows")
            .into_iter()
            .map(|row| row.get::<_, String>(0))
            .collect::<Vec<_>>();
        assert_eq!(
            federation_migrations,
            vec![
                "20260712000000_create_federation_schema.sql".to_owned(),
                "20260719000000_create_federation_bark_projection.sql".to_owned(),
            ]
        );

        connection_handle.abort();
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pre_bark_postgres_database_upgrades_with_only_the_forward_migration() {
        let test_id = unique_federation_test_id("pre_bark_upgrade");
        let db_url = test_db_url_with_schema(&test_id);
        let current = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("create current PostgreSQL database");
        drop(current);

        let config = PgConfig::from(db_url.as_str());
        let schema = config.schema.clone().expect("test schema");
        let (client, connection) = connect(&config.url, NoTls)
            .await
            .expect("postgres connection");
        let connection_handle = tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(&format!(
                r#"
                DROP TABLE "{schema}".federation_bark_observations;
                DROP TABLE "{schema}".federation_bark_transitions;
                DROP TABLE "{schema}".federation_bark_reservations;
                DROP TABLE "{schema}".federation_bark_derivation_allocations;
                DROP TABLE "{schema}".federation_bark_vtxos;
                DROP TABLE "{schema}".federation_bark_actions;
                DELETE FROM "{schema}".migrations
                WHERE name = '20260719000000_create_federation_bark_projection.sql';
                "#
            ))
            .await
            .expect("rewind fixture to the pre-Bark schema");

        let upgraded = MintPgDatabase::new(db_url.as_str())
            .await
            .expect("apply only the forward Bark migration");
        drop(upgraded);
        let rows = client
            .query(
                r#"
                SELECT table_name
                FROM information_schema.tables
                WHERE table_schema = $1
                  AND table_name LIKE 'federation_bark_%'
                ORDER BY table_name
                "#,
                &[&schema],
            )
            .await
            .expect("query upgraded Bark tables");
        assert_eq!(rows.len(), 6);
        let proof_count = client
            .query_one(
                r#"
                SELECT COUNT(*)
                FROM information_schema.tables
                WHERE table_schema = $1 AND table_name = 'proof'
                "#,
                &[&schema],
            )
            .await
            .expect("query historical standalone table")
            .get::<_, i64>(0);
        assert_eq!(proof_count, 1);
        connection_handle.abort();
    }

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
