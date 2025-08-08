//! FFI Database bindings

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::WalletDatabase as CdkWalletDatabase;
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
    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), FfiError>;
    
    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError>;
    
    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError>;
    
    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<String, Option<MintInfo>>, FfiError>;
    
    /// Update mint url
    async fn update_mint_url(&self, old_mint_url: MintUrl, new_mint_url: MintUrl) -> Result<(), FfiError>;

    // Keyset Management
    /// Add mint keyset to storage
    async fn add_mint_keysets(&self, mint_url: MintUrl, keysets: Vec<KeySetInfo>) -> Result<(), FfiError>;
    
    /// Get mint keysets for mint url
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, FfiError>;
    
    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError>;

    // Mint Quote Management
    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: std::sync::Arc<MintQuote>) -> Result<(), FfiError>;
    
    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<std::sync::Arc<MintQuote>>, FfiError>;
    
    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<std::sync::Arc<MintQuote>>, FfiError>;
    
    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError>;

    // Melt Quote Management
    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: std::sync::Arc<MeltQuote>) -> Result<(), FfiError>;
    
    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<std::sync::Arc<MeltQuote>>, FfiError>;
    
    /// Get melt quotes from storage
    async fn get_melt_quotes(&self) -> Result<Vec<std::sync::Arc<MeltQuote>>, FfiError>;
    
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
    async fn update_proofs(&self, added: Vec<ProofInfo>, removed_ys: Vec<PublicKey>) -> Result<(), FfiError>;
    
    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError>;
    
    /// Update proofs state in storage
    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), FfiError>;

    // Keyset Counter Management
    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<(), FfiError>;
    
    /// Get current Keyset counter
    async fn get_keyset_counter(&self, keyset_id: Id) -> Result<Option<u32>, FfiError>;

    // Transaction Management
    /// Add transaction to storage
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError>;
    
    /// Get transaction from storage
    async fn get_transaction(&self, transaction_id: TransactionId) -> Result<Option<Transaction>, FfiError>;
    
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

// WalletDatabaseBridge removed - FFI trait will be used directly

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabase trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<CdkWalletSqliteDatabase>,
}

impl WalletSqliteDatabase {
    /// Get the inner CDK database instance (not exposed through FFI)
    pub(crate) fn inner(&self) -> Arc<CdkWalletSqliteDatabase> {
        self.inner.clone()
    }
}

#[uniffi::export]
impl WalletSqliteDatabase {

    /// Create a new WalletSqliteDatabase with the given work directory
    #[uniffi::constructor]
    pub async fn new(work_dir: String) -> Result<Arc<Self>, FfiError> {
        crate::runtime::block_on(async move {
            let db = CdkWalletSqliteDatabase::new(work_dir.as_str())
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(Arc::new(Self {
                inner: Arc::new(db),
            }))
        })
    }

    /// Create an in-memory database
    #[uniffi::constructor]
    pub async fn new_in_memory() -> Result<Arc<Self>, FfiError> {
        crate::runtime::block_on(async move {
            let db = cdk_sqlite::wallet::memory::empty()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(Arc::new(Self {
                inner: Arc::new(db),
            }))
        })
    }
}

#[uniffi::export]
#[async_trait::async_trait]
impl WalletDatabase for WalletSqliteDatabase {
    // Mint Management
    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.try_into()?;
            let cdk_mint_info = mint_info.map(Into::into);
            self.inner
                .add_mint(cdk_mint_url, cdk_mint_info)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.try_into()?;
            self.inner
                .remove_mint(cdk_mint_url)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.try_into()?;
            let result = self
                .inner
                .get_mint(cdk_mint_url)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(Into::into))
        })
    }
    
    async fn get_mints(&self) -> Result<HashMap<String, Option<MintInfo>>, FfiError> {
        crate::runtime::block_on(async move {
            let result = self
                .inner
                .get_mints()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.map(Into::into)))
                .collect())
        })
    }
    
    async fn update_mint_url(&self, old_mint_url: MintUrl, new_mint_url: MintUrl) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_old_mint_url = old_mint_url.try_into()?;
            let cdk_new_mint_url = new_mint_url.try_into()?;
            self.inner
                .update_mint_url(cdk_old_mint_url, cdk_new_mint_url)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Keyset Management
    async fn add_mint_keysets(&self, mint_url: MintUrl, keysets: Vec<KeySetInfo>) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.try_into()?;
            let cdk_keysets: Vec<cdk_common::nuts::KeySetInfo> =
                keysets.into_iter().map(Into::into).collect();
            self.inner
                .add_mint_keysets(cdk_mint_url, cdk_keysets)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.try_into()?;
            let result = self
                .inner
                .get_mint_keysets(cdk_mint_url)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(|keysets| keysets.into_iter().map(Into::into).collect()))
        })
    }
    
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = keyset_id.into();
            let result = self
                .inner
                .get_keyset_by_id(&cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(Into::into))
        })
    }

    // Mint Quote Management
    async fn add_mint_quote(&self, quote: std::sync::Arc<MintQuote>) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_quote = quote.inner.clone();
            self.inner
                .add_mint_quote(cdk_quote)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<std::sync::Arc<MintQuote>>, FfiError> {
        crate::runtime::block_on(async move {
            let result = self
                .inner
                .get_mint_quote(&quote_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(|q| std::sync::Arc::new(q.into())))
        })
    }
    
    async fn get_mint_quotes(&self) -> Result<Vec<std::sync::Arc<MintQuote>>, FfiError> {
        crate::runtime::block_on(async move {
            let result = self
                .inner
                .get_mint_quotes()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.into_iter().map(|q| std::sync::Arc::new(q.into())).collect())
        })
    }
    
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            self.inner
                .remove_mint_quote(&quote_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Melt Quote Management
    async fn add_melt_quote(&self, quote: std::sync::Arc<MeltQuote>) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_quote = quote.inner.clone();
            self.inner
                .add_melt_quote(cdk_quote)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<std::sync::Arc<MeltQuote>>, FfiError> {
        crate::runtime::block_on(async move {
            let result = self
                .inner
                .get_melt_quote(&quote_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(|q| std::sync::Arc::new(q.into())))
        })
    }
    
    async fn get_melt_quotes(&self) -> Result<Vec<std::sync::Arc<MeltQuote>>, FfiError> {
        crate::runtime::block_on(async move {
            let result = self
                .inner
                .get_melt_quotes()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.into_iter().map(|q| std::sync::Arc::new(q.into())).collect())
        })
    }
    
    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            self.inner
                .remove_melt_quote(&quote_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Keys Management
    async fn add_keys(&self, _keyset: KeySet) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            // Convert FFI KeySet to cashu::KeySet
            // For now, return an error as this requires complex conversion
            Err(FfiError::Database {
                msg: "add_keys not fully implemented yet".to_string()
            })
        })
    }
    
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = id.into();
            let result = self
                .inner
                .get_keys(&cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(Into::into))
        })
    }
    
    async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = id.into();
            self.inner
                .remove_keys(&cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Proof Management  
    async fn update_proofs(&self, added: Vec<ProofInfo>, removed_ys: Vec<PublicKey>) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            // Convert FFI types to CDK types
            let cdk_added: Result<Vec<cdk_common::common::ProofInfo>, FfiError> = added.into_iter()
                .map(|info| {
                    Ok::<cdk_common::common::ProofInfo, FfiError>(cdk_common::common::ProofInfo {
                        proof: info.proof.inner.clone(),
                        y: info.y.try_into()?,
                        mint_url: info.mint_url.try_into()?,
                        state: info.state.into(),
                        spending_condition: info.spending_condition.map(|sc| sc.try_into()).transpose()?,
                        unit: info.unit.into(),
                    })
                })
                .collect();
            let cdk_added = cdk_added?;
            
            let cdk_removed_ys: Result<Vec<cdk_common::nuts::PublicKey>, FfiError> = removed_ys.into_iter()
                .map(|pk| pk.try_into())
                .collect();
            let cdk_removed_ys = cdk_removed_ys?;
            
            self.inner
                .update_proofs(cdk_added, cdk_removed_ys)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
            let cdk_unit = unit.map(Into::into);
            let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());
            let cdk_spending_conditions: Option<Vec<cdk_common::nuts::SpendingConditions>> = spending_conditions.map(|sc| {
                sc.into_iter().map(|c| c.try_into()).collect::<Result<Vec<_>, FfiError>>()
            }).transpose()?;
            
            let result = self
                .inner
                .get_proofs(cdk_mint_url, cdk_unit, cdk_state, cdk_spending_conditions)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            
            Ok(result.into_iter().map(Into::into).collect())
        })
    }
    
    async fn update_proofs_state(&self, ys: Vec<PublicKey>, state: State) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_ys: Result<Vec<cdk_common::nuts::PublicKey>, FfiError> = ys.into_iter()
                .map(|pk| pk.try_into())
                .collect();
            let cdk_ys = cdk_ys?;
            let cdk_state = state.into();
            
            self.inner
                .update_proofs_state(cdk_ys, cdk_state)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Keyset Counter Management
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = keyset_id.into();
            self.inner
                .increment_keyset_counter(&cdk_id, count)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_keyset_counter(&self, keyset_id: Id) -> Result<Option<u32>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = keyset_id.into();
            self.inner
                .get_keyset_counter(&cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }

    // Transaction Management
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            // Convert FFI Transaction to CDK Transaction using TryFrom
            let cdk_transaction: cdk_common::wallet::Transaction = transaction.try_into()?;
            
            self.inner
                .add_transaction(cdk_transaction)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
    
    async fn get_transaction(&self, transaction_id: TransactionId) -> Result<Option<Transaction>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = transaction_id.try_into()?;
            let result = self
                .inner
                .get_transaction(cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            Ok(result.map(Into::into))
        })
    }
    
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, FfiError> {
        crate::runtime::block_on(async move {
            let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
            let cdk_direction = direction.map(Into::into);
            let cdk_unit = unit.map(Into::into);
            
            let result = self
                .inner
                .list_transactions(cdk_mint_url, cdk_direction, cdk_unit)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;
            
            Ok(result.into_iter().map(Into::into).collect())
        })
    }
    
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        crate::runtime::block_on(async move {
            let cdk_id = transaction_id.try_into()?;
            self.inner
                .remove_transaction(cdk_id)
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })
        })
    }
}

// Helper function removed - FFI trait will be used directly
