use std::sync::Arc;

use cdk_postgres::PgConnectionPool;

use crate::{
    CurrencyUnit, FfiError, FfiWalletSQLDatabase, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions,
    Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

#[derive(uniffi::Object)]
pub struct WalletPostgresDatabase {
    inner: Arc<FfiWalletSQLDatabase<PgConnectionPool>>,
}

// Keep a long-lived Tokio runtime for Postgres-created resources so that
// background tasks (e.g., tokio-postgres connection drivers spawned during
// construction) are not tied to a short-lived, ad-hoc runtime.
#[cfg(feature = "postgres")]
static PG_RUNTIME: once_cell::sync::OnceCell<tokio::runtime::Runtime> =
    once_cell::sync::OnceCell::new();

#[cfg(feature = "postgres")]
fn pg_runtime() -> &'static tokio::runtime::Runtime {
    PG_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("cdk-ffi-pg")
            .build()
            .expect("failed to build pg runtime")
    })
}

#[uniffi::export]
impl WalletPostgresDatabase {
    /// Create a new Postgres-backed wallet database
    /// Requires cdk-ffi to be built with feature "postgres".
    /// Example URL:
    ///  "host=localhost user=test password=test dbname=testdb port=5433 schema=wallet sslmode=prefer"
    #[cfg(feature = "postgres")]
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new(url: String) -> Result<Arc<Self>, FfiError> {
        let inner = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(
                    async move { cdk_postgres::new_wallet_pg_database(url.as_str()).await },
                )
            }),
            // Important: use a process-long runtime so background connection tasks stay alive.
            Err(_) => pg_runtime()
                .block_on(async move { cdk_postgres::new_wallet_pg_database(url.as_str()).await }),
        }
        .map_err(FfiError::internal)?;
        Ok(Arc::new(WalletPostgresDatabase {
            inner: FfiWalletSQLDatabase::new(inner),
        }))
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletPostgresDatabase);
