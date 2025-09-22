//! FFI Database bindings

use std::collections::HashMap;
use std::sync::Arc;

use cdk::cdk_database::WalletDatabase as CdkWalletDatabase;
use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;

use crate::error::FfiError;
use crate::types::*;

/// FFI-compatible trait for wallet database operations
/// This trait mirrors the CDK WalletDatabase trait but uses FFI-compatible types
#[uniffi::export(with_foreign)]
#[async_trait::async_trait]
pub trait WalletDatabase: Send + Sync {
    // Mint Management
    /// Add Mint to storage
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError>;

    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError>;

    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError>;

    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError>;

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

    /// Get mint keysets for mint url
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError>;

    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError>;

    // Mint Quote Management
    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError>;

    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError>;

    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError>;

    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError>;

    // Melt Quote Management
    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError>;

    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError>;

    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError>;

    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError>;

    // Keys Management
    /// Add Keys to storage
    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError>;

    /// Get Keys from storage
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError>;

    /// Remove Keys from storage
    async fn remove_keys(&self, id: Id) -> Result<(), FfiError>;

    // Proof Management
    /// Update the proofs in storage by adding new proofs or removing proofs by their Y value
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError>;

    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError>;

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

    /// Remove transaction from storage
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError>;
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
impl CdkWalletDatabase for WalletDatabaseBridge {
    type Err = cdk::cdk_database::Error;

    // Mint Management
    async fn add_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
        mint_info: Option<cdk::nuts::MintInfo>,
    ) -> Result<(), Self::Err> {
        let ffi_mint_url = mint_url.into();
        let ffi_mint_info = mint_info.map(Into::into);

        self.ffi_db
            .add_mint(ffi_mint_url, ffi_mint_info)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_mint(&self, mint_url: cdk::mint_url::MintUrl) -> Result<(), Self::Err> {
        let ffi_mint_url = mint_url.into();
        self.ffi_db
            .remove_mint(ffi_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<cdk::nuts::MintInfo>, Self::Err> {
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
    ) -> Result<HashMap<cdk::mint_url::MintUrl, Option<cdk::nuts::MintInfo>>, Self::Err> {
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

    async fn update_mint_url(
        &self,
        old_mint_url: cdk::mint_url::MintUrl,
        new_mint_url: cdk::mint_url::MintUrl,
    ) -> Result<(), Self::Err> {
        let ffi_old_mint_url = old_mint_url.into();
        let ffi_new_mint_url = new_mint_url.into();
        self.ffi_db
            .update_mint_url(ffi_old_mint_url, ffi_new_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Keyset Management
    async fn add_mint_keysets(
        &self,
        mint_url: cdk::mint_url::MintUrl,
        keysets: Vec<cdk::nuts::KeySetInfo>,
    ) -> Result<(), Self::Err> {
        let ffi_mint_url = mint_url.into();
        let ffi_keysets: Vec<KeySetInfo> = keysets.into_iter().map(Into::into).collect();

        self.ffi_db
            .add_mint_keysets(ffi_mint_url, ffi_keysets)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_mint_keysets(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<Vec<cdk::nuts::KeySetInfo>>, Self::Err> {
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
    ) -> Result<Option<cdk::nuts::KeySetInfo>, Self::Err> {
        let ffi_id = (*keyset_id).into();
        let result = self
            .ffi_db
            .get_keyset_by_id(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    // Mint Quote Management
    async fn add_mint_quote(&self, quote: cdk::wallet::MintQuote) -> Result<(), Self::Err> {
        let ffi_quote = quote.into();
        self.ffi_db
            .add_mint_quote(ffi_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_mint_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MintQuote>, Self::Err> {
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

    async fn get_mint_quotes(&self) -> Result<Vec<cdk::wallet::MintQuote>, Self::Err> {
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

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.ffi_db
            .remove_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Melt Quote Management
    async fn add_melt_quote(&self, quote: cdk::wallet::MeltQuote) -> Result<(), Self::Err> {
        let ffi_quote = quote.into();
        self.ffi_db
            .add_melt_quote(ffi_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MeltQuote>, Self::Err> {
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

    async fn get_melt_quotes(&self) -> Result<Vec<cdk::wallet::MeltQuote>, Self::Err> {
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

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.ffi_db
            .remove_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Keys Management
    async fn add_keys(&self, keyset: cdk::nuts::KeySet) -> Result<(), Self::Err> {
        let ffi_keyset: KeySet = keyset.into();
        self.ffi_db
            .add_keys(ffi_keyset)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_keys(&self, id: &cdk::nuts::Id) -> Result<Option<cdk::nuts::Keys>, Self::Err> {
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

    async fn remove_keys(&self, id: &cdk::nuts::Id) -> Result<(), Self::Err> {
        let ffi_id = (*id).into();
        self.ffi_db
            .remove_keys(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Proof Management
    async fn update_proofs(
        &self,
        added: Vec<cdk::types::ProofInfo>,
        removed_ys: Vec<cdk::nuts::PublicKey>,
    ) -> Result<(), Self::Err> {
        let ffi_added: Vec<ProofInfo> = added.into_iter().map(Into::into).collect();
        let ffi_removed_ys: Vec<PublicKey> = removed_ys.into_iter().map(Into::into).collect();

        self.ffi_db
            .update_proofs(ffi_added, ffi_removed_ys)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_proofs(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
        spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>>,
    ) -> Result<Vec<cdk::types::ProofInfo>, Self::Err> {
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
                    proof: info.proof.inner.clone(),
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

    async fn update_proofs_state(
        &self,
        ys: Vec<cdk::nuts::PublicKey>,
        state: cdk::nuts::State,
    ) -> Result<(), Self::Err> {
        let ffi_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();
        let ffi_state = state.into();

        self.ffi_db
            .update_proofs_state(ffi_ys, ffi_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Keyset Counter Management
    async fn increment_keyset_counter(
        &self,
        keyset_id: &cdk::nuts::Id,
        count: u32,
    ) -> Result<u32, Self::Err> {
        let ffi_id = (*keyset_id).into();
        self.ffi_db
            .increment_keyset_counter(ffi_id, count)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    // Transaction Management
    async fn add_transaction(
        &self,
        transaction: cdk::wallet::types::Transaction,
    ) -> Result<(), Self::Err> {
        let ffi_transaction = transaction.into();
        self.ffi_db
            .add_transaction(ffi_transaction)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_transaction(
        &self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<Option<cdk::wallet::types::Transaction>, Self::Err> {
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
    ) -> Result<Vec<cdk::wallet::types::Transaction>, Self::Err> {
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

    async fn remove_transaction(
        &self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<(), Self::Err> {
        let ffi_id = transaction_id.into();
        self.ffi_db
            .remove_transaction(ffi_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }
}

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabase trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<CdkWalletSqliteDatabase>,
}

impl WalletSqliteDatabase {
    // No additional methods needed beyond the trait implementation
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
            inner: Arc::new(db),
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
            inner: Arc::new(db),
        }))
    }
}

#[uniffi::export(async_runtime = "tokio")]
#[async_trait::async_trait]
impl WalletDatabase for WalletSqliteDatabase {
    // Mint Management
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let cdk_mint_info = mint_info.map(Into::into);
        self.inner
            .add_mint(cdk_mint_url, cdk_mint_info)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        self.inner
            .remove_mint(cdk_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
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

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError> {
        let cdk_old_mint_url = old_mint_url.try_into()?;
        let cdk_new_mint_url = new_mint_url.try_into()?;
        self.inner
            .update_mint_url(cdk_old_mint_url, cdk_new_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Keyset Management
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let cdk_keysets: Vec<cdk::nuts::KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        self.inner
            .add_mint_keysets(cdk_mint_url, cdk_keysets)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
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

    // Mint Quote Management
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError> {
        let cdk_quote = quote.try_into()?;
        self.inner
            .add_mint_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
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

    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner
            .remove_mint_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Melt Quote Management
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        let cdk_quote = quote.try_into()?;
        self.inner
            .add_melt_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
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

    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        self.inner
            .remove_melt_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Keys Management
    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        // Convert FFI KeySet to cdk::nuts::KeySet
        let cdk_keyset: cdk::nuts::KeySet = keyset.try_into()?;
        self.inner
            .add_keys(cdk_keyset)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
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

    async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        let cdk_id = id.into();
        self.inner
            .remove_keys(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Proof Management
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError> {
        // Convert FFI types to CDK types
        let cdk_added: Result<Vec<cdk::types::ProofInfo>, FfiError> = added
            .into_iter()
            .map(|info| {
                Ok::<cdk::types::ProofInfo, FfiError>(cdk::types::ProofInfo {
                    proof: info.proof.inner.clone(),
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

        self.inner
            .update_proofs(cdk_added, cdk_removed_ys)
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

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;
        let cdk_state = state.into();

        self.inner
            .update_proofs_state(cdk_ys, cdk_state)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Keyset Counter Management
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError> {
        let cdk_id = keyset_id.into();
        self.inner
            .increment_keyset_counter(&cdk_id, count)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Transaction Management
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        // Convert FFI Transaction to CDK Transaction using TryFrom
        let cdk_transaction: cdk::wallet::types::Transaction = transaction.try_into()?;

        self.inner
            .add_transaction(cdk_transaction)
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

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        let cdk_id = transaction_id.try_into()?;
        self.inner
            .remove_transaction(cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }
}

/// Helper function to create a CDK database from the FFI trait
pub fn create_cdk_database_from_ffi(
    ffi_db: Arc<dyn WalletDatabase>,
) -> Arc<dyn CdkWalletDatabase<Err = cdk::cdk_database::Error> + Send + Sync> {
    Arc::new(WalletDatabaseBridge::new(ffi_db))
}
