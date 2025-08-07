//! FFI Database bindings

use std::sync::Arc;

use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;

use crate::error::FfiError;

/// FFI-compatible WalletSqliteDatabase
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<CdkWalletSqliteDatabase>,
}

#[uniffi::export]
impl WalletSqliteDatabase {
    /// Create a new WalletSqliteDatabase with the given work directory
    #[uniffi::constructor]
    pub async fn new(work_dir: String) -> Result<Self, FfiError> {
        let db = CdkWalletSqliteDatabase::new(work_dir.as_str())
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(Self {
            inner: Arc::new(db),
        })
    }

    /// Create an in-memory database
    #[uniffi::constructor]
    pub async fn new_in_memory() -> Result<Self, FfiError> {
        let db = cdk_sqlite::wallet::memory::empty()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(Self {
            inner: Arc::new(db),
        })
    }
}

// Separate impl block for internal methods
impl WalletSqliteDatabase {
    /// Get the inner database for use in wallet creation (internal use only)
    pub(crate) fn get_inner(
        &self,
    ) -> Arc<
        dyn cdk_common::database::WalletDatabase<Err = cdk_common::database::Error> + Send + Sync,
    > {
        self.inner.clone()
    }
}
