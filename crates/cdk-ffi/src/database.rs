//! FFI Database bindings

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use cdk_common::database::{
    DbTransactionFinalizer, DynWalletDatabaseTransaction, KVStoreDatabase as CdkKVStoreDatabase,
    WalletDatabase as CdkWalletDatabase, WalletDatabaseTransaction as CdkWalletDatabaseTransaction,
};
use cdk_common::task::spawn;
use cdk_sql_common::pool::DatabasePool;
use cdk_sql_common::SQLWalletDatabase;
use tokio::sync::Mutex;

use crate::error::FfiError;
#[cfg(feature = "postgres")]
use crate::postgres::WalletPostgresDatabase;
use crate::sqlite::WalletSqliteDatabase;
use crate::types::*;

/// FFI-compatible wallet database trait (read-only operations + begin_db_transaction)
/// This trait mirrors the CDK WalletDatabase trait structure
#[uniffi::export]
#[async_trait::async_trait]
pub trait WalletDatabase: Send + Sync {
    /// Begin a database transaction
    async fn begin_db_transaction(&self)
        -> Result<Arc<WalletDatabaseTransactionWrapper>, FfiError>;

    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError>;

    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError>;

    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError>;

    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError>;

    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError>;

    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError>;

    /// Get unissued mint quotes from storage
    /// Returns bolt11 quotes where nothing has been issued yet (amount_issued = 0) and all bolt12 quotes.
    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError>;

    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError>;

    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError>;

    /// Get Keys from storage
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError>;

    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError>;

    /// Get proofs by Y values
    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, FfiError>;

    /// Get balance efficiently using SQL aggregation
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError>;

    /// Get transaction from storage
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError>;

    /// List transactions from storage
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, FfiError>;

    /// Read a value from the KV store
    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError>;

    /// List keys in a namespace
    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError>;
}

/// FFI-compatible transaction trait for wallet database write operations
/// This trait mirrors the CDK WalletDatabaseTransaction trait but uses FFI-compatible types
#[uniffi::export(with_foreign)]
#[async_trait::async_trait]
pub trait WalletDatabaseTransaction: Send + Sync {
    /// Commit the transaction
    async fn commit(self: Arc<Self>) -> Result<(), FfiError>;

    /// Rollback the transaction
    async fn rollback(self: Arc<Self>) -> Result<(), FfiError>;

    // Mint Management
    /// Add Mint to storage
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError>;

    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError>;

    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError>;

    // Keyset Management
    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError>;

    /// Get mint keyset by id (transaction-scoped)
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError>;

    /// Get Keys from storage (transaction-scoped)
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError>;

    // Mint Quote Management
    /// Get mint quote from storage (transaction-scoped, with locking)
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError>;

    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError>;

    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError>;

    // Melt Quote Management
    /// Get melt quote from storage (transaction-scoped)
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError>;

    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError>;

    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError>;

    // Keys Management
    /// Add Keys to storage
    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError>;

    /// Remove Keys from storage
    async fn remove_keys(&self, id: Id) -> Result<(), FfiError>;

    // Proof Management
    /// Get proofs from storage (transaction-scoped, with locking)
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError>;

    /// Update the proofs in storage by adding new proofs or removing proofs by their Y value
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError>;

    /// Update proofs state in storage
    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError>;

    // Keyset Counter Management
    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError>;

    // Transaction Management
    /// Add transaction to storage
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError>;

    /// Remove transaction from storage
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError>;

    // KV Store Methods
    /// Read a value from the KV store
    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError>;

    /// Write a value to the KV store
    async fn kv_write(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), FfiError>;

    /// Remove a value from the KV store
    async fn kv_remove(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<(), FfiError>;

    /// List keys in the KV store
    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError>;
    
    // async fn add_p2pk_key(&self, pubkey: PublicKey, derivation_path: String, derivation_index: u32) -> Result<(), FfiError>;
    // async fn get_p2pk_key(&self, pubkey: PublicKey) -> Result<Option<P2PKSigningKey>, FfiError>;
    // async fn list_p2pk_keys(&self) -> Result<Vec<P2PKSigningKey>, FfiError>;
}

/// Wallet database transaction wrapper
#[derive(uniffi::Object)]
pub struct WalletDatabaseTransactionWrapper {
    inner: Arc<dyn WalletDatabaseTransaction>,
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletDatabaseTransactionWrapper {
    /// Commit the transaction
    pub async fn commit(&self) -> Result<(), FfiError> {
        self.inner.clone().commit().await
    }

    /// Rollback the transaction
    pub async fn rollback(&self) -> Result<(), FfiError> {
        self.inner.clone().rollback().await
    }

    /// Add Mint to storage
    pub async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError> {
        self.inner.add_mint(mint_url, mint_info).await
    }

    /// Remove Mint from storage
    pub async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        self.inner.remove_mint(mint_url).await
    }

    /// Update mint url
    pub async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError> {
        self.inner.update_mint_url(old_mint_url, new_mint_url).await
    }

    /// Add mint keyset to storage
    pub async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError> {
        self.inner.add_mint_keysets(mint_url, keysets).await
    }

    /// Get mint keyset by id (transaction-scoped)
    pub async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        self.inner.get_keyset_by_id(keyset_id).await
    }

    /// Get Keys from storage (transaction-scoped)
    pub async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        self.inner.get_keys(id).await
    }

    /// Get mint quote from storage (transaction-scoped, with locking)
    pub async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        self.inner.get_mint_quote(quote_id).await
    }

    /// Add mint quote to storage
    pub async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError> {
        self.inner.add_mint_quote(quote).await
    }

    /// Remove mint quote from storage
    pub async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner.remove_mint_quote(quote_id).await
    }

    /// Get melt quote from storage (transaction-scoped)
    pub async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        self.inner.get_melt_quote(quote_id).await
    }

    /// Add melt quote to storage
    pub async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        self.inner.add_melt_quote(quote).await
    }

    /// Remove melt quote from storage
    pub async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner.remove_melt_quote(quote_id).await
    }

    /// Add Keys to storage
    pub async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        self.inner.add_keys(keyset).await
    }

    /// Remove Keys from storage
    pub async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        self.inner.remove_keys(id).await
    }

    /// Get proofs from storage (transaction-scoped, with locking)
    pub async fn get_proofs(
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

    /// Update the proofs in storage by adding new proofs or removing proofs by their Y value
    pub async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError> {
        self.inner.update_proofs(added, removed_ys).await
    }

    /// Update proofs state in storage
    pub async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        self.inner.update_proofs_state(ys, state).await
    }

    /// Increment Keyset counter
    pub async fn increment_keyset_counter(
        &self,
        keyset_id: Id,
        count: u32,
    ) -> Result<u32, FfiError> {
        self.inner.increment_keyset_counter(keyset_id, count).await
    }

    /// Add transaction to storage
    pub async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        self.inner.add_transaction(transaction).await
    }

    /// Remove transaction from storage
    pub async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        self.inner.remove_transaction(transaction_id).await
    }

    /// Read a value from the KV store
    pub async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError> {
        self.inner
            .kv_read(primary_namespace, secondary_namespace, key)
            .await
    }

    /// Write a value to the KV store
    pub async fn kv_write(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), FfiError> {
        self.inner
            .kv_write(primary_namespace, secondary_namespace, key, value)
            .await
    }

    /// Remove a value from the KV store
    pub async fn kv_remove(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<(), FfiError> {
        self.inner
            .kv_remove(primary_namespace, secondary_namespace, key)
            .await
    }

    /// List keys in the KV store
    pub async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError> {
        self.inner
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }
}

/// Internal bridge trait to convert from the FFI trait to the CDK database trait
/// This allows us to bridge between the UniFFI trait and the CDK's internal database trait
struct WalletDatabaseBridge {
    ffi_db: Arc<dyn WalletDatabase>,
}

impl WalletDatabaseBridge {
    fn new(ffi_db: Arc<dyn WalletDatabase>) -> Self {
        Self { ffi_db }
    }
}

impl std::fmt::Debug for WalletDatabaseBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WalletDatabaseBridge")
    }
}

#[async_trait::async_trait]
impl cdk_common::database::KVStoreDatabase for WalletDatabaseBridge {
    type Err = cdk::cdk_database::Error;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        self.ffi_db
            .kv_read(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err> {
        self.ffi_db
            .kv_list(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }
}

#[async_trait::async_trait]
impl CdkWalletDatabase<cdk::cdk_database::Error> for WalletDatabaseBridge {
    // Mint Management
    async fn get_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<cdk::nuts::MintInfo>, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.into();
        let result = self
            .ffi_db
            .get_mint(ffi_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    async fn get_mints(
        &self,
    ) -> Result<
        HashMap<cdk::mint_url::MintUrl, Option<cdk::nuts::MintInfo>>,
        cdk::cdk_database::Error,
    > {
        let result = self
            .ffi_db
            .get_mints()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        let mut cdk_result = HashMap::new();
        for (ffi_mint_url, mint_info_opt) in result {
            let cdk_url = ffi_mint_url
                .try_into()
                .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))?;
            cdk_result.insert(cdk_url, mint_info_opt.map(Into::into));
        }
        Ok(cdk_result)
    }

    // Keyset Management
    async fn get_mint_keysets(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<Vec<cdk::nuts::KeySetInfo>>, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.into();
        let result = self
            .ffi_db
            .get_mint_keysets(ffi_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(|keysets| keysets.into_iter().map(Into::into).collect()))
    }

    async fn get_keyset_by_id(
        &self,
        keyset_id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::KeySetInfo>, cdk::cdk_database::Error> {
        let ffi_id = (*keyset_id).into();
        let result = self
            .ffi_db
            .get_keyset_by_id(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    // Mint Quote Management
    async fn get_mint_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_db
            .get_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .transpose()?)
    }

    async fn get_mint_quotes(
        &self,
    ) -> Result<Vec<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_db
            .get_mint_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<cdk::wallet::MintQuote>, Self::Err> {
        let result = self
            .ffi_db
            .get_unissued_mint_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    // Melt Quote Management
    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MeltQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_db
            .get_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .transpose()?)
    }

    async fn get_melt_quotes(
        &self,
    ) -> Result<Vec<cdk::wallet::MeltQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_db
            .get_melt_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    // Keys Management
    async fn get_keys(
        &self,
        id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::Keys>, cdk::cdk_database::Error> {
        let ffi_id: Id = (*id).into();
        let result = self
            .ffi_db
            .get_keys(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        // Convert FFI Keys back to CDK Keys using TryFrom
        result
            .map(|ffi_keys| {
                ffi_keys
                    .try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .transpose()
    }

    // Proof Management
    async fn get_proofs(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
        spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>>,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.map(Into::into);
        let ffi_unit = unit.map(Into::into);
        let ffi_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let ffi_spending_conditions =
            spending_conditions.map(|sc| sc.into_iter().map(Into::into).collect());

        let result = self
            .ffi_db
            .get_proofs(ffi_mint_url, ffi_unit, ffi_state, ffi_spending_conditions)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        // Convert back to CDK ProofInfo
        let cdk_result: Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> = result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: FfiError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                })
            })
            .collect();

        cdk_result
    }

    async fn get_proofs_by_ys(
        &self,
        ys: Vec<cdk::nuts::PublicKey>,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let ffi_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();

        let result = self
            .ffi_db
            .get_proofs_by_ys(ffi_ys)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        // Convert back to CDK ProofInfo
        let cdk_result: Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> = result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: FfiError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                })
            })
            .collect();

        cdk_result
    }

    async fn get_balance(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
    ) -> Result<u64, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.map(Into::into);
        let ffi_unit = unit.map(Into::into);
        let ffi_state = state.map(|s| s.into_iter().map(Into::into).collect());

        self.ffi_db
            .get_balance(ffi_mint_url, ffi_unit, ffi_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Transaction Management
    async fn get_transaction(
        &self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<Option<cdk::wallet::types::Transaction>, cdk::cdk_database::Error> {
        let ffi_id = transaction_id.into();
        let result = self
            .ffi_db
            .get_transaction(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .map(|tx| tx.try_into())
            .transpose()
            .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn list_transactions(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        direction: Option<cdk::wallet::types::TransactionDirection>,
        unit: Option<cdk::nuts::CurrencyUnit>,
    ) -> Result<Vec<cdk::wallet::types::Transaction>, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.map(Into::into);
        let ffi_direction = direction.map(Into::into);
        let ffi_unit = unit.map(Into::into);

        let result = self
            .ffi_db
            .list_transactions(ffi_mint_url, ffi_direction, ffi_unit)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .into_iter()
            .map(|tx| tx.try_into())
            .collect::<Result<Vec<_>, FfiError>>()
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn begin_db_transaction(
        &self,
    ) -> Result<
        Box<dyn CdkWalletDatabaseTransaction<cdk::cdk_database::Error> + Send + Sync>,
        cdk::cdk_database::Error,
    > {
        let ffi_tx = self
            .ffi_db
            .begin_db_transaction()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        Ok(Box::new(WalletDatabaseTransactionBridge {
            ffi_tx,
            is_finalized: false,
        }))
    }

    async fn add_p2pk_key(
        &self,
        pubkey: &cdk::nuts::PublicKey,
        derivation_path: String,
        derivation_index: u32,
    ) -> Result<(), cdk::cdk_database::Error> {
        todo!("Implement add_p2pk_key");
        Ok(())
        // let ffi_pubkey = pubkey.into();
        // let result = self.ffi_tx.add_p2pk_key(ffi_pubkey, derivation_path, derivation_index).await.map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        // Ok(result)
    }

    async fn list_p2pk_keys(&self) -> Result<Vec<cdk_common::wallet::P2PKSigningKey>, cdk::cdk_database::Error> {
        todo!("Implement list_p2pk_keys");
        Ok(vec![])
        // let result = self.ffi_tx.list_p2pk_keys().await.map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        // Ok(result.into_iter().map(Into::into).collect())
    }

    async fn get_p2pk_key(
        & self,
        pubkey: &cdk::nuts::PublicKey,
    ) -> Result<Option<cdk_common::wallet::P2PKSigningKey>, cdk::cdk_database::Error> {
        todo!("Implement get_p2pk_key");
        Ok(None)
        // let ffi_pubkey = pubkey.into();
        // let result = self.ffi_tx.get_p2pk_key(ffi_pubkey).await.map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        // Ok(result.map(|q| {
        //         q.try_into()
        //             .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
        //     })
        //     .transpose()?)
    }  
}

/// Transaction bridge for FFI wallet database
struct WalletDatabaseTransactionBridge {
    ffi_tx: Arc<WalletDatabaseTransactionWrapper>,
    is_finalized: bool,
}

#[async_trait::async_trait]
impl CdkWalletDatabaseTransaction<cdk::cdk_database::Error> for WalletDatabaseTransactionBridge {
    async fn add_mint(
        &mut self,
        mint_url: cdk::mint_url::MintUrl,
        mint_info: Option<cdk::nuts::MintInfo>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.into();
        let ffi_mint_info = mint_info.map(Into::into);
        self.ffi_tx
            .add_mint(ffi_mint_url, ffi_mint_info)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_mint(
        &mut self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.into();
        self.ffi_tx
            .remove_mint(ffi_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_mint_url(
        &mut self,
        old_mint_url: cdk::mint_url::MintUrl,
        new_mint_url: cdk::mint_url::MintUrl,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_old_mint_url = old_mint_url.into();
        let ffi_new_mint_url = new_mint_url.into();
        self.ffi_tx
            .update_mint_url(ffi_old_mint_url, ffi_new_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_mint_keysets(
        &mut self,
        mint_url: cdk::mint_url::MintUrl,
        keysets: Vec<cdk::nuts::KeySetInfo>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.into();
        let ffi_keysets: Vec<KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        self.ffi_tx
            .add_mint_keysets(ffi_mint_url, ffi_keysets)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_mint_quote(
        &mut self,
        quote: cdk::wallet::MintQuote,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_quote = quote.into();
        self.ffi_tx
            .add_mint_quote(ffi_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_mint_quote(&mut self, quote_id: &str) -> Result<(), cdk::cdk_database::Error> {
        self.ffi_tx
            .remove_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_melt_quote(
        &mut self,
        quote: cdk::wallet::MeltQuote,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_quote = quote.into();
        self.ffi_tx
            .add_melt_quote(ffi_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_melt_quote(&mut self, quote_id: &str) -> Result<(), cdk::cdk_database::Error> {
        self.ffi_tx
            .remove_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_keys(
        &mut self,
        keyset: cdk::nuts::KeySet,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_keyset: KeySet = keyset.into();
        self.ffi_tx
            .add_keys(ffi_keyset)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_keys(&mut self, id: &cdk::nuts::Id) -> Result<(), cdk::cdk_database::Error> {
        let ffi_id = (*id).into();
        self.ffi_tx
            .remove_keys(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_proofs(
        &mut self,
        added: Vec<cdk::types::ProofInfo>,
        removed_ys: Vec<cdk::nuts::PublicKey>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_added: Vec<ProofInfo> = added.into_iter().map(Into::into).collect();
        let ffi_removed_ys: Vec<PublicKey> = removed_ys.into_iter().map(Into::into).collect();
        self.ffi_tx
            .update_proofs(ffi_added, ffi_removed_ys)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_proofs_state(
        &mut self,
        ys: Vec<cdk::nuts::PublicKey>,
        state: cdk::nuts::State,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();
        let ffi_state = state.into();
        self.ffi_tx
            .update_proofs_state(ffi_ys, ffi_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn increment_keyset_counter(
        &mut self,
        keyset_id: &cdk::nuts::Id,
        count: u32,
    ) -> Result<u32, cdk::cdk_database::Error> {
        let ffi_id = (*keyset_id).into();
        self.ffi_tx
            .increment_keyset_counter(ffi_id, count)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_transaction(
        &mut self,
        transaction: cdk::wallet::types::Transaction,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_transaction = transaction.into();
        self.ffi_tx
            .add_transaction(ffi_transaction)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_transaction(
        &mut self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<(), cdk::cdk_database::Error> {
        let ffi_id = transaction_id.into();
        self.ffi_tx
            .remove_transaction(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Read methods needed during transactions
    async fn get_keyset_by_id(
        &mut self,
        keyset_id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::KeySetInfo>, cdk::cdk_database::Error> {
        let ffi_id = (*keyset_id).into();
        let result = self
            .ffi_tx
            .get_keyset_by_id(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    async fn get_keys(
        &mut self,
        id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::Keys>, cdk::cdk_database::Error> {
        let ffi_id = (*id).into();
        let result = self
            .ffi_tx
            .get_keys(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        match result {
            Some(keys) => Ok(Some(keys.try_into().map_err(|e: FfiError| {
                cdk::cdk_database::Error::Database(e.to_string().into())
            })?)),
            None => Ok(None),
        }
    }

    async fn get_mint_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_tx
            .get_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .transpose()?)
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MeltQuote>, cdk::cdk_database::Error> {
        let result = self
            .ffi_tx
            .get_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into()
                    .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .transpose()?)
    }

    async fn get_proofs(
        &mut self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
        spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>>,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let ffi_mint_url = mint_url.map(Into::into);
        let ffi_unit = unit.map(Into::into);
        let ffi_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let ffi_spending_conditions =
            spending_conditions.map(|sc| sc.into_iter().map(Into::into).collect());

        let result = self
            .ffi_tx
            .get_proofs(ffi_mint_url, ffi_unit, ffi_state, ffi_spending_conditions)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        // Convert back to CDK ProofInfo
        let cdk_result: Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> = result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: FfiError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: FfiError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                })
            })
            .collect();

        cdk_result
    }
    
}

#[async_trait::async_trait]
impl cdk_common::database::KVStoreTransaction<cdk::cdk_database::Error>
    for WalletDatabaseTransactionBridge
{
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, cdk::cdk_database::Error> {
        self.ffi_tx
            .kv_read(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), cdk::cdk_database::Error> {
        self.ffi_tx
            .kv_write(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
                value.to_vec(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.ffi_tx
            .kv_remove(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, cdk::cdk_database::Error> {
        self.ffi_tx
            .kv_list(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }
}

#[async_trait::async_trait]
impl DbTransactionFinalizer for WalletDatabaseTransactionBridge {
    type Err = cdk::cdk_database::Error;

    async fn commit(mut self: Box<Self>) -> Result<(), cdk::cdk_database::Error> {
        self.is_finalized = true;
        let tx = self.ffi_tx.clone();
        tx.commit()
            .await
            .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), cdk::cdk_database::Error> {
        self.is_finalized = true;
        let tx = self.ffi_tx.clone();
        tx.rollback()
            .await
            .map_err(|e: FfiError| cdk::cdk_database::Error::Database(e.to_string().into()))
    }
}

impl Drop for WalletDatabaseTransactionBridge {
    fn drop(&mut self) {
        if !self.is_finalized {
            let tx = self.ffi_tx.clone();
            spawn(async move {
                let _ = tx.rollback().await;
            });
        }
    }
}

pub(crate) struct FfiWalletSQLDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    inner: SQLWalletDatabase<RM>,
}

impl<RM> FfiWalletSQLDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    /// Creates a new instance
    pub fn new(inner: SQLWalletDatabase<RM>) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

/// Transaction wrapper for FFI
pub(crate) struct FfiWalletTransaction {
    tx: Arc<Mutex<Option<DynWalletDatabaseTransaction>>>,
    is_finalized: AtomicBool,
}

impl Drop for FfiWalletTransaction {
    fn drop(&mut self) {
        if !self.is_finalized.load(std::sync::atomic::Ordering::SeqCst) {
            let tx = self.tx.clone();
            spawn(async move {
                if let Some(s) = tx.lock().await.take() {
                    let _ = s.rollback().await;
                }
            });
        }
    }
}

impl FfiWalletTransaction {
    pub fn new(tx: DynWalletDatabaseTransaction) -> Arc<Self> {
        Arc::new(Self {
            tx: Arc::new(Mutex::new(Some(tx))),
            is_finalized: false.into(),
        })
    }
}

// Implement WalletDatabaseFfi trait - only read methods + begin_db_transaction
#[async_trait::async_trait]
impl<RM> WalletDatabase for FfiWalletSQLDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    async fn begin_db_transaction(
        &self,
    ) -> Result<Arc<WalletDatabaseTransactionWrapper>, FfiError> {
        let tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;

        Ok(Arc::new(WalletDatabaseTransactionWrapper {
            inner: FfiWalletTransaction::new(tx),
        }))
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, FfiError> {
        let cdk_ys: Vec<cdk::nuts::PublicKey> = ys
            .into_iter()
            .map(|y| y.try_into())
            .collect::<Result<Vec<_>, FfiError>>()?;

        let result = self
            .inner
            .get_proofs_by_ys(cdk_ys)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let result = self
            .inner
            .get_mint(cdk_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError> {
        let result = self
            .inner
            .get_mints()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result
            .into_iter()
            .map(|(k, v)| (k.into(), v.map(Into::into)))
            .collect())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let result = self
            .inner
            .get_mint_keysets(cdk_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(|keysets| keysets.into_iter().map(Into::into).collect()))
    }

    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        let cdk_id = keyset_id.into();
        let result = self
            .inner
            .get_keyset_by_id(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        let result = self
            .inner
            .get_mint_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(|q| q.into()))
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        let result = self
            .inner
            .get_mint_quotes()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        let result = self
            .inner
            .get_unissued_mint_quotes()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        let result = self
            .inner
            .get_melt_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(|q| q.into()))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError> {
        let result = self
            .inner
            .get_melt_quotes()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        let cdk_id = id.into();
        let result = self
            .inner
            .get_keys(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_unit = unit.map(Into::into);
        let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let cdk_spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>> =
            spending_conditions
                .map(|sc| {
                    sc.into_iter()
                        .map(|c| c.try_into())
                        .collect::<Result<Vec<_>, FfiError>>()
                })
                .transpose()?;

        let result = self
            .inner
            .get_proofs(cdk_mint_url, cdk_unit, cdk_state, cdk_spending_conditions)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_unit = unit.map(Into::into);
        let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());

        self.inner
            .get_balance(cdk_mint_url, cdk_unit, cdk_state)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError> {
        let cdk_id = transaction_id.try_into()?;
        let result = self
            .inner
            .get_transaction(cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_direction = direction.map(Into::into);
        let cdk_unit = unit.map(Into::into);

        let result = self
            .inner
            .list_transactions(cdk_mint_url, cdk_direction, cdk_unit)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError> {
        self.inner
            .kv_read(&primary_namespace, &secondary_namespace, &key)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError> {
        self.inner
            .kv_list(&primary_namespace, &secondary_namespace)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }
}

// Implement WalletDatabaseTransactionFfi trait - all write methods
#[async_trait::async_trait]
impl WalletDatabaseTransaction for FfiWalletTransaction {
    async fn commit(self: Arc<Self>) -> Result<(), FfiError> {
        self.is_finalized
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.tx
            .lock()
            .await
            .take()
            .ok_or(FfiError::Database {
                msg: "Transaction already finalized".to_owned(),
            })?
            .commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn rollback(self: Arc<Self>) -> Result<(), FfiError> {
        self.is_finalized
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.tx
            .lock()
            .await
            .take()
            .ok_or(FfiError::Database {
                msg: "Transaction already finalized".to_owned(),
            })?
            .rollback()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;

        let cdk_mint_url = mint_url.try_into()?;
        let cdk_mint_info = mint_info.map(Into::into);
        tx.add_mint(cdk_mint_url, cdk_mint_info)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_mint_url = mint_url.try_into()?;
        tx.remove_mint(cdk_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_old_mint_url = old_mint_url.try_into()?;
        let cdk_new_mint_url = new_mint_url.try_into()?;
        tx.update_mint_url(cdk_old_mint_url, cdk_new_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_mint_url = mint_url.try_into()?;
        let cdk_keysets: Vec<cdk::nuts::KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        tx.add_mint_keysets(cdk_mint_url, cdk_keysets)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_id = keyset_id.into();
        let result = tx
            .get_keyset_by_id(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_id = id.into();
        let result = tx
            .get_keys(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(Into::into))
    }

    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let result = tx
            .get_mint_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(|q| q.into()))
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_quote = quote.try_into()?;
        tx.add_mint_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.remove_mint_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let result = tx
            .get_melt_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result.map(|q| q.into()))
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_quote = quote.try_into()?;
        tx.add_melt_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.remove_melt_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_keyset: cdk::nuts::KeySet = keyset.try_into()?;
        tx.add_keys(cdk_keyset)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_id = id.into();
        tx.remove_keys(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;

        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_unit = unit.map(Into::into);
        let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let cdk_spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>> =
            spending_conditions
                .map(|sc| {
                    sc.into_iter()
                        .map(|c| c.try_into())
                        .collect::<Result<Vec<_>, FfiError>>()
                })
                .transpose()?;

        let result = tx
            .get_proofs(cdk_mint_url, cdk_unit, cdk_state, cdk_spending_conditions)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;

        let cdk_added: Result<Vec<cdk::types::ProofInfo>, FfiError> = added
            .into_iter()
            .map(|info| {
                Ok::<cdk::types::ProofInfo, FfiError>(cdk::types::ProofInfo {
                    proof: info.proof.try_into()?,
                    y: info.y.try_into()?,
                    mint_url: info.mint_url.try_into()?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()?,
                    unit: info.unit.into(),
                })
            })
            .collect();
        let cdk_added = cdk_added?;

        let cdk_removed_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            removed_ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_removed_ys = cdk_removed_ys?;

        tx.update_proofs(cdk_added, cdk_removed_ys)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;
        let cdk_state = state.into();

        tx.update_proofs_state(cdk_ys, cdk_state)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_id = keyset_id.into();
        tx.increment_keyset_counter(&cdk_id, count)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;

        let cdk_transaction: cdk::wallet::types::Transaction = transaction.try_into()?;

        tx.add_transaction(cdk_transaction)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        let cdk_id = transaction_id.try_into()?;
        tx.remove_transaction(cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.kv_read(&primary_namespace, &secondary_namespace, &key)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn kv_write(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.kv_write(&primary_namespace, &secondary_namespace, &key, &value)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn kv_remove(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<(), FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.kv_remove(&primary_namespace, &secondary_namespace, &key)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError> {
        let mut tx_guard = self.tx.lock().await;
        let tx = tx_guard.as_mut().ok_or(FfiError::Database {
            msg: "Transaction already finalized".to_owned(),
        })?;
        tx.kv_list(&primary_namespace, &secondary_namespace)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }
    
    // async fn add_p2pk_key(&self, pubkey: PublicKey, derivation_path: String, derivation_index: u32) -> Result<(), FfiError> {
    //     todo!("Implement add_p2pk_key");
    //     Ok(())
    // }
    // async fn get_p2pk_key(&self, pubkey: PublicKey) -> Result<Option<P2PKSigningKey>, FfiError> {
    //     todo!("Implement get_p2pk_key");
    //     Ok(None)
    // }
    // async fn list_p2pk_keys(&self) -> Result<Vec<P2PKSigningKey>, FfiError> {
    //     todo!("Implement list_p2pk_keys");
    //     Ok(vec![])
    // }
}

/// FFI-safe database type enum
#[derive(uniffi::Enum, Clone)]
pub enum WalletDbBackend {
    Sqlite {
        path: String,
    },
    #[cfg(feature = "postgres")]
    Postgres {
        url: String,
    },
}

/// Factory helpers returning a CDK wallet database behind the FFI trait
#[uniffi::export]
pub fn create_wallet_db(backend: WalletDbBackend) -> Result<Arc<dyn WalletDatabase>, FfiError> {
    match backend {
        WalletDbBackend::Sqlite { path } => {
            let sqlite = WalletSqliteDatabase::new(path)?;
            Ok(sqlite as Arc<dyn WalletDatabase>)
        }
        #[cfg(feature = "postgres")]
        WalletDbBackend::Postgres { url } => {
            let pg = WalletPostgresDatabase::new(url)?;
            Ok(pg as Arc<dyn WalletDatabase>)
        }
    }
}

/// Helper function to create a CDK database from the FFI trait
pub fn create_cdk_database_from_ffi(
    ffi_db: Arc<dyn WalletDatabase>,
) -> Arc<dyn CdkWalletDatabase<cdk::cdk_database::Error> + Send + Sync> {
    Arc::new(WalletDatabaseBridge::new(ffi_db))
}
