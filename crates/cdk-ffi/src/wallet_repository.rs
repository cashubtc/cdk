//! FFI WalletRepository bindings

use std::collections::HashMap;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::wallet_repository::{
    WalletRepository as CdkWalletRepository, WalletRepositoryBuilder,
};

use crate::error::FfiError;
use crate::types::*;
use crate::wallet::RateLimit;

/// Configuration for creating a wallet repository.
///
/// Rate limiting is a repository-wide setting: every wallet the repository
/// hands out shares one limiter, so there is no per-wallet equivalent.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletRepositoryConfig {
    /// Proxy used by every mint operation. Omit for a direct connection.
    #[uniffi(default = None)]
    pub proxy_url: Option<String>,
    /// Client-side request pacing to start with. Omit it to keep the built-in
    /// default.
    #[uniffi(default = None)]
    pub rate_limit: Option<RateLimit>,
}

/// FFI-compatible WalletRepository
#[derive(uniffi::Object)]
pub struct WalletRepository {
    inner: Arc<CdkWalletRepository>,
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
    /// Create a new WalletRepository from locally persisted wallet state.
    ///
    /// Construction does not make network requests to configured mints.
    ///
    /// Accepts a `WalletStore` which can be:
    /// - `Sqlite { path }` — built-in Rust SQLite backend
    /// - `Postgres { url }` — built-in Rust Postgres backend
    /// - `Custom { db }` — foreign-language implementation of `WalletDatabase`
    #[uniffi::constructor]
    pub fn new(mnemonic: String, store: crate::database::WalletStore) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(store)?;

        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let wallet = rt.block_on(async move {
            WalletRepositoryBuilder::new()
                .localstore(localstore)
                .seed(seed)
                .build()
                .await
        })?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Create a new WalletRepository with proxy configuration.
    ///
    /// Construction restores locally persisted wallet state without making
    /// network requests to configured mints. The proxy is used by subsequent
    /// mint operations.
    #[uniffi::constructor]
    pub fn new_with_proxy(
        mnemonic: String,
        store: crate::database::WalletStore,
        proxy_url: String,
    ) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(store)?;

        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        // Parse proxy URL
        let proxy_url = url::Url::parse(&proxy_url)
            .map_err(|e| FfiError::internal(format!("Invalid URL: {}", e)))?;

        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let wallet = rt.block_on(async move {
            WalletRepositoryBuilder::new()
                .localstore(localstore)
                .seed(seed)
                .proxy_url(proxy_url)
                .build()
                .await
        })?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Create a new WalletRepository with proxy and rate-limit configuration.
    ///
    /// Construction restores locally persisted wallet state without making
    /// network requests to configured mints.
    #[uniffi::constructor]
    pub fn new_with_config(
        mnemonic: String,
        store: crate::database::WalletStore,
        config: WalletRepositoryConfig,
    ) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(store)?;

        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let proxy_url = config
            .proxy_url
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|e| FfiError::internal(format!("Invalid URL: {}", e)))?;

        let rate_limit = config
            .rate_limit
            .as_ref()
            .map(RateLimit::to_config)
            .transpose()?;

        let rt = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let wallet = rt.block_on(async move {
            let mut builder = WalletRepositoryBuilder::new()
                .localstore(localstore)
                .seed(seed);

            if let Some(proxy_url) = proxy_url {
                builder = builder.proxy_url(proxy_url);
            }

            builder = match rate_limit {
                Some(Some(config)) => builder.with_rate_limiting_config(config),
                Some(None) => builder.with_rate_limiting_disabled(),
                None => builder,
            };

            builder.build().await
        })?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Set metadata cache TTL (time-to-live) in seconds for a specific mint
    ///
    /// Controls how long cached mint metadata (keysets, keys, mint info) is considered fresh
    /// before requiring a refresh from the mint server for a specific mint.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint URL to set the TTL for
    /// * `ttl_secs` - Optional TTL in seconds. If None, cache never expires.
    pub async fn set_metadata_cache_ttl_for_mint(
        &self,
        mint_url: MintUrl,
        ttl_secs: Option<u64>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let wallets = self.inner.get_wallets().await;

        if let Some(wallet) = wallets.iter().find(|w| w.mint_url == cdk_mint_url) {
            let ttl = ttl_secs.map(std::time::Duration::from_secs);
            wallet.set_metadata_cache_ttl(ttl);
            Ok(())
        } else {
            Err(FfiError::internal(format!(
                "Mint not found: {}",
                cdk_mint_url
            )))
        }
    }

    /// Set metadata cache TTL (time-to-live) in seconds for all mints
    ///
    /// Controls how long cached mint metadata is considered fresh for all mints
    /// in this WalletRepository.
    ///
    /// # Arguments
    ///
    /// * `ttl_secs` - Optional TTL in seconds. If None, cache never expires for any mint.
    pub async fn set_metadata_cache_ttl_for_all_mints(&self, ttl_secs: Option<u64>) {
        let wallets = self.inner.get_wallets().await;
        let ttl = ttl_secs.map(std::time::Duration::from_secs);

        for wallet in wallets.iter() {
            wallet.set_metadata_cache_ttl(ttl);
        }
    }

    /// Add a mint to this WalletRepository
    pub async fn create_wallet(
        &self,
        mint_url: MintUrl,
        unit: Option<CurrencyUnit>,
        target_proof_count: Option<u32>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;

        let config = target_proof_count.map(|count| {
            cdk::wallet::wallet_repository::WalletConfig::new()
                .with_target_proof_count(count as usize)
        });

        let unit_enum = unit.unwrap_or(CurrencyUnit::Sat);

        self.inner
            .create_wallet(cdk_mint_url, unit_enum.into(), config)
            .await?;

        Ok(())
    }

    /// Get the wallet for a mint URL and unit, creating it if it does not exist
    ///
    /// Unlike `create_wallet`, an existing wallet is returned untouched: its
    /// configuration is not replaced.
    pub async fn get_or_create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        target_proof_count: Option<u32>,
    ) -> Result<Arc<crate::wallet::Wallet>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;

        let config = target_proof_count.map(|count| {
            cdk::wallet::wallet_repository::WalletConfig::new()
                .with_target_proof_count(count as usize)
        });

        let wallet = self
            .inner
            .get_or_create_wallet(cdk_mint_url, unit.into(), config)
            .await?;

        Ok(Arc::new(crate::wallet::Wallet::from_inner(Arc::new(
            wallet,
        ))))
    }

    /// Remove mint from WalletRepository
    pub async fn remove_wallet(
        &self,
        mint_url: MintUrl,
        currency_unit: CurrencyUnit,
    ) -> Result<(), FfiError> {
        // 1. Convert MintUrl safely without unwrap()
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url
            .try_into()
            .map_err(|_| FfiError::internal("invalid mint url"))?; // Map the error to your FfiError type

        // 2. Await the inner call and propagate its result with '?'
        self.inner
            .remove_wallet(cdk_mint_url, currency_unit.into())
            .await
            .map_err(|e| e.into()) // Ensure the inner error can convert to FfiError
    }

    /// Move all wallets for a mint to a new mint URL
    ///
    /// Persists the migration through the wallet database, then rebuilds every
    /// affected wallet against the new URL so their connectors target the new
    /// endpoint. Wallet objects obtained before this call keep the old URL and
    /// must be fetched again from the repository. Returns the rebuilt wallets.
    ///
    /// Intended for when a mint moves or announces an alternative endpoint,
    /// for example through the NUT-06 `urls` field. The caller is responsible
    /// for verifying that the new endpoint serves the same mint (its keysets
    /// match) before migrating.
    pub async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<Vec<Arc<crate::wallet::Wallet>>, FfiError> {
        let cdk_old_mint_url: cdk::mint_url::MintUrl = old_mint_url.try_into()?;
        let cdk_new_mint_url: cdk::mint_url::MintUrl = new_mint_url.try_into()?;

        let wallets = self
            .inner
            .update_mint_url(cdk_old_mint_url, cdk_new_mint_url)
            .await?;

        Ok(wallets
            .into_iter()
            .map(|wallet| Arc::new(crate::wallet::Wallet::from_inner(Arc::new(wallet))))
            .collect())
    }

    /// Wait until the rate-limit budgets drawn down by every wallet in this
    /// repository have been handed to storage.
    ///
    /// Await this before dropping the repository on shutdown. Without it,
    /// persistence is best effort and a rebuild can outrun the detached
    /// writer, so every rebuilt wallet starts with a full burst against the
    /// mint's rate cap.
    pub async fn flush_rate_limits(&self) {
        self.inner.flush_rate_limits().await;
    }

    /// Change client-side request rate limiting for every wallet here.
    ///
    /// Pacing is repository-wide because one limiter is shared, which is why
    /// `create_wallet` and `get_or_create_wallet` take no rate limit: a
    /// per-wallet value would silently reconfigure its siblings. Repository
    /// construction makes no network requests, so calling this immediately
    /// after `new` is equivalent to configuring it through `new_with_config`.
    ///
    /// Returns an error if a `Custom` value has a zero field.
    pub fn set_rate_limit(&self, rate_limit: RateLimit) -> Result<(), FfiError> {
        self.inner.set_rate_limiting_config(rate_limit.to_config()?);
        Ok(())
    }

    /// Whether this repository is pacing requests right now.
    ///
    /// Wallets reached through a proxy or Tor are built with a custom client,
    /// so they report false even while this is true.
    pub fn is_rate_limited(&self) -> bool {
        self.inner.is_rate_limited()
    }

    /// Check if mint is in wallet
    pub async fn has_mint(&self, mint_url: MintUrl) -> bool {
        if let Ok(cdk_mint_url) = mint_url.try_into() {
            self.inner.has_mint(&cdk_mint_url).await
        } else {
            false
        }
    }

    /// Get the NUT-27 mint backup public key as hex.
    pub fn mint_backup_public_key(&self) -> Result<String, FfiError> {
        let keys = self.inner.backup_keys()?;
        Ok(keys.public_key().to_hex())
    }

    /// Backup the current mint list to Nostr relays using NUT-27.
    pub async fn backup_mints(
        &self,
        relays: Vec<String>,
        options: BackupOptions,
    ) -> Result<BackupResult, FfiError> {
        let result = self.inner.backup_mints(relays, options.into()).await?;
        Ok(result.into())
    }

    /// Restore the mint list from Nostr relays using NUT-27.
    pub async fn restore_mints(
        &self,
        relays: Vec<String>,
        add_mints: bool,
        options: RestoreOptions,
    ) -> Result<RestoreResult, FfiError> {
        let result = self
            .inner
            .restore_mints(relays, add_mints, options.into())
            .await?;
        Ok(result.into())
    }

    /// Fetch the NUT-27 mint backup without adding mints to the repository.
    pub async fn fetch_mint_backup(
        &self,
        relays: Vec<String>,
        options: RestoreOptions,
    ) -> Result<MintBackup, FfiError> {
        let backup = self.inner.fetch_backup(relays, options.into()).await?;
        Ok(backup.into())
    }

    /// Get wallet balances for all mints
    pub async fn get_balances(&self) -> Result<HashMap<WalletKey, Amount>, FfiError> {
        let balances = self.inner.get_balances().await?;
        let mut balance_map = HashMap::new();
        for (wallet_key, amount) in balances {
            balance_map.insert(wallet_key.into(), amount.into());
        }
        Ok(balance_map)
    }

    /// Get all wallets from WalletRepository
    pub async fn get_wallets(&self) -> Vec<Arc<crate::wallet::Wallet>> {
        let wallets = self.inner.get_wallets().await;
        wallets
            .into_iter()
            .map(|w| Arc::new(crate::wallet::Wallet::from_inner(Arc::new(w))))
            .collect()
    }

    /// Get a specific wallet from WalletRepository by mint URL
    ///
    /// Returns an error if no wallet exists for the given mint URL.
    pub async fn get_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
    ) -> Result<Arc<crate::wallet::Wallet>, FfiError> {
        let cdk_mint_url: cdk::mint_url::MintUrl = mint_url.try_into()?;
        let unit_cdk: cdk::nuts::CurrencyUnit = unit.into();
        let wallet = self.inner.get_wallet(&cdk_mint_url, &unit_cdk).await?;
        Ok(Arc::new(crate::wallet::Wallet::from_inner(Arc::new(
            wallet,
        ))))
    }

    /// Get token data, including the expected redemption fee, without redeeming it.
    pub async fn get_token_data(
        &self,
        token: Arc<crate::token::Token>,
    ) -> Result<TokenData, FfiError> {
        Ok(self.inner.get_token_data(&token.inner).await?.into())
    }
}

/// Token data FFI type
///
/// Contains information extracted from a parsed token.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TokenData {
    /// The mint URL from the token
    pub mint_url: MintUrl,
    /// The proofs contained in the token
    pub proofs: Vec<crate::types::Proof>,
    /// The memo from the token, if present
    pub memo: Option<String>,
    /// Value of token in smallest unit
    pub value: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Fee to redeem (None if unknown)
    pub redeem_fee: Option<Amount>,
}

impl From<cdk::wallet::TokenData> for TokenData {
    fn from(data: cdk::wallet::TokenData) -> Self {
        Self {
            mint_url: data.mint_url.into(),
            proofs: data.proofs.into_iter().map(Into::into).collect(),
            memo: data.memo,
            value: data.value.into(),
            unit: data.unit.into(),
            redeem_fee: data.redeem_fee.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::custom_wallet_store;
    use crate::sqlite::WalletSqliteDatabase;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn test_repository() -> WalletRepository {
        let db = WalletSqliteDatabase::new_in_memory().expect("in-memory wallet db should open");
        WalletRepository::new(MNEMONIC.to_string(), custom_wallet_store(db))
            .expect("repository should be created")
    }

    fn mint_url() -> MintUrl {
        MintUrl::new("https://mint.example.com".to_string()).expect("valid mint url")
    }

    fn repository_with(rate_limit: Option<RateLimit>) -> Result<WalletRepository, FfiError> {
        let db = WalletSqliteDatabase::new_in_memory().expect("in-memory wallet db should open");
        WalletRepository::new_with_config(
            MNEMONIC.to_string(),
            custom_wallet_store(db),
            WalletRepositoryConfig {
                proxy_url: None,
                rate_limit,
            },
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_with_config_selects_the_starting_rate_limit() {
        for rate_limit in [
            None,
            Some(RateLimit::Default),
            Some(RateLimit::Custom {
                capacity: 5,
                refill_per_minute: 30,
            }),
        ] {
            let repo = repository_with(rate_limit).expect("repository should be created");
            assert!(repo.is_rate_limited());
        }

        let disabled =
            repository_with(Some(RateLimit::Disabled)).expect("repository should be created");
        assert!(!disabled.is_rate_limited());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_with_config_rejects_zero_custom() {
        assert!(repository_with(Some(RateLimit::Custom {
            capacity: 0,
            refill_per_minute: 30,
        }))
        .is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_rate_limit_reaches_wallets_it_handed_out() {
        let repo = repository_with(Some(RateLimit::Disabled)).expect("repository created");
        let wallet = repo
            .get_or_create_wallet(mint_url(), CurrencyUnit::Sat, None)
            .await
            .expect("wallet should be created");
        assert!(!wallet.is_rate_limited());

        repo.set_rate_limit(RateLimit::Default)
            .expect("default is always valid");
        assert!(
            wallet.is_rate_limited(),
            "wallets share the repository limiter"
        );

        repo.set_rate_limit(RateLimit::Disabled)
            .expect("disabled is always valid");
        assert!(!wallet.is_rate_limited());

        assert!(repo
            .set_rate_limit(RateLimit::Custom {
                capacity: 5,
                refill_per_minute: 0,
            })
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_or_create_wallet_creates_a_missing_wallet() {
        let repo = test_repository();

        let wallet = repo
            .get_or_create_wallet(mint_url(), CurrencyUnit::Sat, None)
            .await
            .expect("wallet should be created");

        assert_eq!(wallet.mint_url(), mint_url());
        assert_eq!(wallet.unit(), CurrencyUnit::Sat);
        assert!(repo.get_wallet(mint_url(), CurrencyUnit::Sat).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_or_create_wallet_returns_the_existing_wallet() {
        let repo = test_repository();

        repo.create_wallet(mint_url(), Some(CurrencyUnit::Sat), Some(5))
            .await
            .expect("wallet should be created");

        let wallet = repo
            .get_or_create_wallet(mint_url(), CurrencyUnit::Sat, Some(99))
            .await
            .expect("wallet should be returned");

        assert_eq!(wallet.inner().target_proof_count, 5);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_or_create_wallet_is_keyed_by_unit() {
        let repo = test_repository();

        let sat = repo
            .get_or_create_wallet(mint_url(), CurrencyUnit::Sat, None)
            .await
            .expect("sat wallet should be created");
        let usd = repo
            .get_or_create_wallet(mint_url(), CurrencyUnit::Usd, None)
            .await
            .expect("usd wallet should be created");

        assert_eq!(sat.unit(), CurrencyUnit::Sat);
        assert_eq!(usd.unit(), CurrencyUnit::Usd);
        assert!(repo.get_wallet(mint_url(), CurrencyUnit::Sat).await.is_ok());
        assert!(repo.get_wallet(mint_url(), CurrencyUnit::Usd).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_or_create_wallet_converges_under_concurrency() {
        let repo = Arc::new(test_repository());

        let calls = (1..=8u32).map(|target_proof_count| {
            let repo = repo.clone();
            tokio::spawn(async move {
                repo.get_or_create_wallet(mint_url(), CurrencyUnit::Sat, Some(target_proof_count))
                    .await
                    .expect("wallet should be created")
            })
        });

        let mut counts = Vec::new();
        for call in calls {
            let wallet = call.await.expect("task should not panic");
            counts.push(wallet.inner().target_proof_count);
        }

        // Whichever caller built the wallet, every other caller must observe
        // that same one rather than a wallet of its own.
        assert!(
            counts.windows(2).all(|pair| pair[0] == pair[1]),
            "{counts:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_or_create_wallet_rejects_an_invalid_mint_url() {
        let repo = test_repository();

        // Bindings can build the record directly, bypassing `MintUrl::new`.
        let invalid = MintUrl {
            url: "not a url".to_string(),
        };

        assert!(repo
            .get_or_create_wallet(invalid, CurrencyUnit::Sat, None)
            .await
            .is_err());
    }
}
