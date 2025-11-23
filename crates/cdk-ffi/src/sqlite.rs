use std::collections::HashMap;
use std::sync::Arc;

use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;
use cdk_sqlite::SqliteConnectionManager;

use crate::{
    CurrencyUnit, FfiError, FfiWalletSQLDatabase, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions,
    Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabase trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<FfiWalletSQLDatabase<SqliteConnectionManager>>,
}

#[uniffi::export]
impl WalletSqliteDatabase {
    /// Create a new WalletSqliteDatabase with the given work directory
    #[uniffi::constructor]
    pub fn new(file_path: String) -> Result<Arc<Self>, FfiError> {
        let db = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle
                    .block_on(async move { CdkWalletSqliteDatabase::new(file_path.as_str()).await })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::Database {
                        msg: format!("Failed to create runtime: {}", e),
                    })?
                    .block_on(async move { CdkWalletSqliteDatabase::new(file_path.as_str()).await })
            }
        }
        .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(Arc::new(Self {
            inner: FfiWalletSQLDatabase::new(db),
        }))
    }

    /// Create an in-memory database
    #[uniffi::constructor]
    pub fn new_in_memory() -> Result<Arc<Self>, FfiError> {
        let db = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move { cdk_sqlite::wallet::memory::empty().await })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::Database {
                        msg: format!("Failed to create runtime: {}", e),
                    })?
                    .block_on(async move { cdk_sqlite::wallet::memory::empty().await })
            }
        }
        .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(Arc::new(Self {
            inner: FfiWalletSQLDatabase::new(db),
        }))
    }
}

#[uniffi::export(async_runtime = "tokio")]
#[async_trait::async_trait]
impl WalletDatabase for WalletSqliteDatabase {
    /// Begins a DB transaction
    async fn begin(&self) -> Result<(), FfiError> {
        self.inner.begin().await
    }

    /// Begins a DB transaction
    async fn commit(&self) -> Result<(), FfiError> {
        self.inner.commit().await
    }

    async fn rollback(&self) -> Result<(), FfiError> {
        self.inner.rollback().await
    }

    // Mint Management
    /// Add Mint to storage
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError> {
        self.inner.add_mint(mint_url, mint_info).await
    }

    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        self.inner.remove_mint(mint_url).await
    }

    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        self.inner.get_mint(mint_url).await
    }

    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError> {
        self.inner.get_mints().await
    }

    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError> {
        self.inner.update_mint_url(old_mint_url, new_mint_url).await
    }

    // Keyset Management
    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError> {
        self.inner.add_mint_keysets(mint_url, keysets).await
    }

    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError> {
        self.inner.get_mint_keysets(mint_url).await
    }

    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        self.inner.get_keyset_by_id(keyset_id).await
    }

    // Mint Quote Management
    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError> {
        self.inner.add_mint_quote(quote).await
    }

    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        self.inner.get_mint_quote(quote_id).await
    }

    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        self.inner.get_mint_quotes().await
    }

    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner.remove_mint_quote(quote_id).await
    }

    // Melt Quote Management
    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        self.inner.add_melt_quote(quote).await
    }

    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        self.inner.get_melt_quote(quote_id).await
    }

    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError> {
        self.inner.get_melt_quotes().await
    }

    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner.remove_melt_quote(quote_id).await
    }

    // Keys Management
    /// Add Keys to storage
    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        self.inner.add_keys(keyset).await
    }

    /// Get Keys from storage
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        self.inner.get_keys(id).await
    }

    /// Remove Keys from storage
    async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        self.inner.remove_keys(id).await
    }

    // Proof Management
    /// Update the proofs in storage by adding new proofs or removing proofs by their Y value
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError> {
        self.inner.update_proofs(added, removed_ys).await
    }

    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError> {
        self.inner
            .get_proofs(mint_url, unit, state, spending_conditions)
            .await
    }

    /// Get balance efficiently using SQL aggregation
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError> {
        self.inner.get_balance(mint_url, unit, state).await
    }

    /// Update proofs state in storage
    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        self.inner.update_proofs_state(ys, state).await
    }

    // Keyset Counter Management
    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError> {
        self.inner.increment_keyset_counter(keyset_id, count).await
    }

    // Transaction Management
    /// Add transaction to storage
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        self.inner.add_transaction(transaction).await
    }

    /// Get transaction from storage
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError> {
        self.inner.get_transaction(transaction_id).await
    }

    /// List transactions from storage
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, FfiError> {
        self.inner
            .list_transactions(mint_url, direction, unit)
            .await
    }

    /// Remove transaction from storage
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        self.inner.remove_transaction(transaction_id).await
    }
}
