use std::sync::Arc;

use cdk_common::database::Error as CdkDatabaseError;
use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;

use crate::{
    CurrencyUnit, FfiError, FfiWalletDatabaseWrapper, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, P2PKSigningKey, ProofInfo, ProofState, PublicKey,
    SpendingConditions, Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabaseFfi trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<FfiWalletDatabaseWrapper<CdkWalletSqliteDatabase, CdkDatabaseError>>,
    // Keep the runtime alive so async pool operations work in FFI contexts.
    _runtime: crate::runtime::RuntimeGuard,
}

#[uniffi::export]
impl WalletSqliteDatabase {
    /// Create a new WalletSqliteDatabase with the given work directory
    #[uniffi::constructor]
    pub fn new(file_path: String) -> Result<Arc<Self>, FfiError> {
        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let db = rt
            .block_on(async move { CdkWalletSqliteDatabase::new(file_path.as_str()).await })
            .map_err(FfiError::internal)?;
        Ok(Arc::new(Self {
            inner: FfiWalletDatabaseWrapper::new(db),
            _runtime: rt,
        }))
    }

    /// Create an in-memory database
    #[uniffi::constructor]
    pub fn new_in_memory() -> Result<Arc<Self>, FfiError> {
        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let db = rt
            .block_on(async move { cdk_sqlite::wallet::memory::empty().await })
            .map_err(FfiError::internal)?;
        Ok(Arc::new(Self {
            inner: FfiWalletDatabaseWrapper::new(db),
            _runtime: rt,
        }))
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletSqliteDatabase);
