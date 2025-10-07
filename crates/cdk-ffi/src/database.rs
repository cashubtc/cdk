//! FFI Database bindings

use std::collections::HashMap;
use std::sync::Arc;

use cdk::cdk_database::WalletDatabase as CdkWalletDatabase;

use crate::error::FfiError;
use crate::postgres::WalletPostgresDatabase;
use crate::sqlite::WalletSqliteDatabase;
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

    /// Get balance efficiently using SQL aggregation
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError>;

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
    ) -> Result<u64, Self::Err> {
        let ffi_mint_url = mint_url.map(Into::into);
        let ffi_unit = unit.map(Into::into);
        let ffi_state = state.map(|s| s.into_iter().map(Into::into).collect());

        self.ffi_db
            .get_balance(ffi_mint_url, ffi_unit, ffi_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
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

/// FFI-safe wallet database backend selection
#[derive(uniffi::Enum)]
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
        WalletDbBackend::Postgres { url } => {
            let pg = WalletPostgresDatabase::new(url)?;
            Ok(pg as Arc<dyn WalletDatabase>)
        }
    }
}

/// Helper function to create a CDK database from the FFI trait
pub fn create_cdk_database_from_ffi(
    ffi_db: Arc<dyn WalletDatabase>,
) -> Arc<dyn CdkWalletDatabase<Err = cdk::cdk_database::Error> + Send + Sync> {
    Arc::new(WalletDatabaseBridge::new(ffi_db))
}
