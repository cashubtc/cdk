use std::sync::Arc;

use cdk_common::database::Error as CdkDatabaseError;
use cdk_postgres::WalletPgDatabase;

use crate::{
    CurrencyUnit, FfiError, FfiWalletDatabaseWrapper, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, P2PKSigningKey, ProofInfo, ProofState, PublicKey,
    SpendingConditions, Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

#[derive(uniffi::Object)]
pub struct WalletPostgresDatabase {
    inner: Arc<FfiWalletDatabaseWrapper<WalletPgDatabase, CdkDatabaseError>>,
    // Keep the runtime alive so Postgres background connection tasks survive.
    _runtime: crate::runtime::RuntimeGuard,
}

#[uniffi::export]
impl WalletPostgresDatabase {
    /// Create a new Postgres-backed wallet database
    /// Requires cdk-ffi to be built with feature "postgres".
    /// Example URL:
    ///  "host=localhost user=test password=test dbname=testdb port=5433 schema=wallet sslmode=prefer"
    #[cfg(feature = "postgres")]
    #[uniffi::constructor]
    pub fn new(url: String) -> Result<Arc<Self>, FfiError> {
        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let inner = rt
            .block_on(async move { cdk_postgres::new_wallet_pg_database(url.as_str()).await })
            .map_err(FfiError::internal)?;
        Ok(Arc::new(WalletPostgresDatabase {
            inner: FfiWalletDatabaseWrapper::new(inner),
            _runtime: rt,
        }))
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletPostgresDatabase);
