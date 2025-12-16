use std::collections::HashMap;
use std::sync::Arc;

// Bring the CDK wallet database trait into scope so trait methods resolve on the inner DB
use cdk_postgres::PgConnectionPool;

use crate::{
    CurrencyUnit, FfiError, FfiWalletSQLDatabase, Id, KeySetInfo, Keys, MeltQuote, MintInfo,
    MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions, Transaction,
    TransactionDirection, TransactionId, WalletDatabase, WalletDatabaseTransactionWrapper,
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
}

#[uniffi::export(async_runtime = "tokio")]
#[async_trait::async_trait]
impl WalletDatabase for WalletPostgresDatabase {
    async fn begin_db_transaction(
        &self,
    ) -> Result<Arc<WalletDatabaseTransactionWrapper>, FfiError> {
        self.inner.begin_db_transaction().await
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, FfiError> {
        self.inner.get_proofs_by_ys(ys).await
    }

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        self.inner.get_mint(mint_url).await
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError> {
        self.inner.get_mints().await
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError> {
        self.inner.get_mint_keysets(mint_url).await
    }

    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        self.inner.get_keyset_by_id(keyset_id).await
    }

    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        self.inner.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        self.inner.get_mint_quotes().await
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        self.inner.get_unissued_mint_quotes().await
    }

    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        self.inner.get_melt_quote(quote_id).await
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError> {
        self.inner.get_melt_quotes().await
    }

    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        self.inner.get_keys(id).await
    }

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

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError> {
        self.inner.get_balance(mint_url, unit, state).await
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError> {
        self.inner.get_transaction(transaction_id).await
    }

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

    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError> {
        self.inner
            .kv_read(primary_namespace, secondary_namespace, key)
            .await
    }

    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError> {
        self.inner
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }
}
