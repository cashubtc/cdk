use std::collections::HashMap;
use std::sync::Arc;

// Bring the CDK wallet database trait into scope so trait methods resolve on the inner DB
use cdk_postgres::PgConnectionPool;

use crate::{
    CurrencyUnit, FfiError, FfiWalletSQLDatabase, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions,
    Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

#[derive(uniffi::Object)]
pub struct WalletPostgresDatabase {
    inner: Arc<FfiWalletSQLDatabase<PgConnectionPool>>,
}

// Keep a long-lived Tokio runtime for Postgres-created resources so that
// background tasks (e.g., tokio-postgres connection drivers spawned during
// construction) are not tied to a short-lived, ad-hoc runtime.
#[cfg(feature = "postgres")]
static PG_RUNTIME: once_cell::sync::OnceCell<tokio::runtime::Runtime> =
    once_cell::sync::OnceCell::new();

#[cfg(feature = "postgres")]
fn pg_runtime() -> &'static tokio::runtime::Runtime {
    PG_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("cdk-ffi-pg")
            .build()
            .expect("failed to build pg runtime")
    })
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
        let inner = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(
                    async move { cdk_postgres::new_wallet_pg_database(url.as_str()).await },
                )
            }),
            // Important: use a process-long runtime so background connection tasks stay alive.
            Err(_) => pg_runtime()
                .block_on(async move { cdk_postgres::new_wallet_pg_database(url.as_str()).await }),
        }
        .map_err(|e| FfiError::Database { msg: e.to_string() })?;
        Ok(Arc::new(WalletPostgresDatabase {
            inner: FfiWalletSQLDatabase::new(inner),
        }))
    }

    fn clone_as_trait(&self) -> Arc<dyn WalletDatabase> {
        // Safety: UniFFI objects are reference counted and Send+Sync via Arc
        let obj: Arc<dyn WalletDatabase> = Arc::new(WalletPostgresDatabase {
            inner: self.inner.clone(),
        });
        obj
    }
}

#[uniffi::export(async_runtime = "tokio")]
#[async_trait::async_trait]
impl WalletDatabase for WalletPostgresDatabase {
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
