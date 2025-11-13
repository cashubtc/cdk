use std::collections::HashMap;
use std::sync::Arc;

use cdk_sqlite::wallet::WalletSqliteDatabase as CdkWalletSqliteDatabase;

use crate::{
    CurrencyUnit, FfiError, Id, KeySet, KeySetInfo, Keys, MeltQuote, MintInfo, MintQuote, MintUrl,
    ProofInfo, ProofState, PublicKey, SpendingConditions, Transaction, TransactionDirection,
    TransactionId, WalletDatabase,
};

/// FFI-compatible WalletSqliteDatabase implementation that implements the WalletDatabase trait
#[derive(uniffi::Object)]
pub struct WalletSqliteDatabase {
    inner: Arc<CdkWalletSqliteDatabase>,
}
use cdk::cdk_database::WalletDatabase as CdkWalletDatabase;

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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_mint(cdk_mint_url, cdk_mint_info)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.remove_mint(cdk_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.update_mint_url(cdk_old_mint_url, cdk_new_mint_url)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_mint_keysets(cdk_mint_url, cdk_keysets)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_mint_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.remove_mint_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Melt Quote Management
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        let cdk_quote = quote.try_into()?;
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_melt_quote(cdk_quote)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.remove_melt_quote(&quote_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Keys Management
    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        // Convert FFI KeySet to cdk::nuts::KeySet
        let cdk_keyset: cdk::nuts::KeySet = keyset.try_into()?;
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_keys(cdk_keyset)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.remove_keys(&cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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

        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.update_proofs(cdk_added, cdk_removed_ys)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;
        let cdk_state = state.into();

        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.update_proofs_state(cdk_ys, cdk_state)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }

    // Keyset Counter Management
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError> {
        let cdk_id = keyset_id.into();
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        let result = tx
            .increment_keyset_counter(&cdk_id, count)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(result)
    }

    // Transaction Management
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        // Convert FFI Transaction to CDK Transaction using TryFrom
        let cdk_transaction: cdk::wallet::types::Transaction = transaction.try_into()?;

        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.add_transaction(cdk_transaction)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
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
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.remove_transaction(cdk_id)
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        tx.commit()
            .await
            .map_err(|e| FfiError::Database { msg: e.to_string() })
    }
}
