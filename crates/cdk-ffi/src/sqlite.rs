use std::sync::Arc;

use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;
use cdk_sqlite::SqliteConnectionManager;

use crate::{
    CurrencyUnit, FfiError, FfiWalletSQLDatabase, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions,
    Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabaseFfi trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<FfiWalletSQLDatabase<SqliteConnectionManager>>,
}

#[uniffi::export]
impl WalletSqliteDatabase {
    /// Create a new WalletSqliteDatabase with the given work directory
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new(file_path: String) -> Result<Arc<Self>, FfiError> {
        let db = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle
                    .block_on(async move { CdkWalletSqliteDatabase::new(file_path.as_str()).await })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move { CdkWalletSqliteDatabase::new(file_path.as_str()).await })
            }
        }
        .map_err(FfiError::internal)?;
        Ok(Arc::new(Self {
            inner: FfiWalletSQLDatabase::new(db),
        }))
    }

    /// Create an in-memory database
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new_in_memory() -> Result<Arc<Self>, FfiError> {
        let db = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move { cdk_sqlite::wallet::memory::empty().await })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move { cdk_sqlite::wallet::memory::empty().await })
            }
        }
        .map_err(FfiError::internal)?;
        Ok(Arc::new(Self {
            inner: FfiWalletSQLDatabase::new(db),
        }))
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletSqliteDatabase);
