//! FFI WalletRepository bindings

use std::collections::HashMap;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::wallet_repository::{
    WalletRepository as CdkWalletRepository, WalletRepositoryBuilder,
};

use crate::error::FfiError;
#[cfg(feature = "federation")]
use crate::federation::{FederationTransport, FederationWalletConfig, FederationWalletInfo};
use crate::types::*;
use crate::wallet::RateLimit;

/// Configuration for creating a wallet repository.
///
/// Rate limiting is a repository-wide setting: every wallet the repository
/// hands out shares one limiter, so there is no per-wallet equivalent.
#[derive(Debug, Clone, Default, uniffi::Record)]
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
    /// Federations imported into this wallet, persisted in its database.
    #[cfg(feature = "federation")]
    federations: cdk::wallet::FederationRegistry,
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
        Self::new_with_config(mnemonic, store, WalletRepositoryConfig::default())
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
        Self::new_with_config(
            mnemonic,
            store,
            WalletRepositoryConfig {
                proxy_url: Some(proxy_url),
                ..Default::default()
            },
        )
    }

    /// Create a new WalletRepository with proxy and rate-limit configuration.
    ///
    /// Construction restores locally persisted wallet state, including
    /// imported federations, without making network requests to configured
    /// mints.
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
        #[cfg(feature = "federation")]
        let federations = cdk::wallet::FederationRegistry::new(localstore.clone());

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

        let repository = Self {
            inner: Arc::new(wallet),
            #[cfg(feature = "federation")]
            federations,
        };
        #[cfg(feature = "federation")]
        rt.block_on(repository.activate_registered_federations())?;
        Ok(repository)
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
        let wallets = self.inner.get_wallets_for_mint(&cdk_mint_url).await;

        if wallets.is_empty() {
            return Err(FfiError::internal(format!(
                "Mint not found: {}",
                cdk_mint_url
            )));
        }

        let ttl = ttl_secs.map(std::time::Duration::from_secs);
        for wallet in wallets {
            wallet.set_metadata_cache_ttl(ttl);
        }
        Ok(())
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
    ///
    /// A mint URL of an imported federation (its local mint URL or any
    /// member's public mint URL) creates the federation's wallet under its
    /// local mint URL.
    pub async fn create_wallet(
        &self,
        mint_url: MintUrl,
        unit: Option<CurrencyUnit>,
        target_proof_count: Option<u32>,
    ) -> Result<(), FfiError> {
        let (cdk_mint_url, config) = self
            .wallet_target(mint_url.try_into()?, target_proof_count)
            .await?;

        let unit_enum = unit.unwrap_or(CurrencyUnit::Sat);

        self.inner
            .create_wallet(cdk_mint_url, unit_enum.into(), config)
            .await?;

        Ok(())
    }

    /// Get the wallet for a mint URL and unit, creating it if it does not exist
    ///
    /// Unlike `create_wallet`, an existing wallet is returned untouched: its
    /// configuration is not replaced. A mint URL that names an existing
    /// wallet through an alias (such as a federation member's public mint
    /// URL) returns that wallet, and a missing wallet for an imported
    /// federation is created under its local mint URL with the federation's
    /// connector.
    pub async fn get_or_create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        target_proof_count: Option<u32>,
    ) -> Result<Arc<crate::wallet::Wallet>, FfiError> {
        let (cdk_mint_url, config) = self
            .wallet_target(mint_url.try_into()?, target_proof_count)
            .await?;

        let wallet = self
            .inner
            .get_or_create_wallet(cdk_mint_url, unit.into(), config)
            .await?;

        Ok(Arc::new(crate::wallet::Wallet::from_inner(Arc::new(
            wallet,
        ))))
    }

    /// Remove mint from WalletRepository
    ///
    /// `mint_url` may be an alias of the wallet's mint URL, such as a
    /// federation member's public mint URL. Only the in-memory wallet is
    /// removed; its proofs, quotes and transactions stay in the database.
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
    ///
    /// True as well for an alias of a wallet's mint URL, such as a federation
    /// member's public mint URL.
    pub async fn has_mint(&self, mint_url: MintUrl) -> bool {
        match cdk::mint_url::MintUrl::try_from(mint_url) {
            Ok(cdk_mint_url) => self.inner.has_mint(&cdk_mint_url).await,
            Err(_) => false,
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

    /// Get wallet balances for wallets matching a specific currency unit
    pub async fn get_balances_for_unit(
        &self,
        unit: CurrencyUnit,
    ) -> Result<HashMap<WalletKey, Amount>, FfiError> {
        let unit_cdk: cdk::nuts::CurrencyUnit = unit.into();
        let balances = self.inner.get_balances_for_unit(&unit_cdk).await?;
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
    /// `mint_url` may be an alias of the wallet's mint URL, such as a
    /// federation member's public mint URL. Returns an error if no wallet
    /// exists for the given mint URL.
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
    ///
    /// A token that names an alias of a wallet's mint URL, such as a
    /// federation member's public mint URL, is decoded by that wallet; the
    /// returned `mint_url` is still the token's own.
    pub async fn get_token_data(
        &self,
        token: Arc<crate::token::Token>,
    ) -> Result<TokenData, FfiError> {
        Ok(self.inner.get_token_data(&token.inner).await?.into())
    }
}

impl WalletRepository {
    /// The mint URL and configuration a wallet for `mint_url` is created
    /// with: for an alias of an imported federation, the federation's local
    /// mint URL and federated configuration; otherwise `mint_url` itself.
    #[cfg_attr(not(feature = "federation"), allow(clippy::unused_async))]
    async fn wallet_target(
        &self,
        mint_url: cdk::mint_url::MintUrl,
        target_proof_count: Option<u32>,
    ) -> Result<
        (
            cdk::mint_url::MintUrl,
            Option<cdk::wallet::wallet_repository::WalletConfig>,
        ),
        FfiError,
    > {
        #[cfg(feature = "federation")]
        if let Some(federation) = self.federations.find_by_mint_url(&mint_url).await? {
            return Ok((
                federation.local_mint_url().clone(),
                Some(federation_wallet_config(&federation, target_proof_count)?),
            ));
        }

        let config = target_proof_count.map(|count| {
            cdk::wallet::wallet_repository::WalletConfig::new()
                .with_target_proof_count(count as usize)
        });
        Ok((mint_url, config))
    }
}

/// Wallet configuration for a federation: its federated connector, its
/// member URLs as mint URL aliases and an optional target proof count.
#[cfg(feature = "federation")]
fn federation_wallet_config(
    federation: &cdk::wallet::RegisteredFederation,
    target_proof_count: Option<u32>,
) -> Result<cdk::wallet::wallet_repository::WalletConfig, FfiError> {
    let config = federation.wallet_config_with_transport(FederationTransport::default())?;
    Ok(match target_proof_count {
        Some(count) => config.with_target_proof_count(count as usize),
        None => config,
    })
}

#[cfg(feature = "federation")]
impl WalletRepository {
    /// Install a federated connector, with every member URL as a mint URL
    /// alias, for every active unit of a registered federation.
    async fn activate_federation(
        &self,
        federation: &cdk::wallet::RegisteredFederation,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        let info = FederationWalletInfo::try_from(federation)?;
        let wallet_config = federation_wallet_config(federation, target_proof_count)?;
        for unit in federation.active_units() {
            self.inner
                .set_mint_config(
                    federation.local_mint_url().clone(),
                    unit,
                    wallet_config.clone(),
                )
                .await?;
        }
        Ok(info)
    }

    /// Reactivate every federation persisted in the wallet database.
    async fn activate_registered_federations(&self) -> Result<(), FfiError> {
        for federation in self.federations.list().await? {
            self.activate_federation(&federation, None).await?;
        }
        Ok(())
    }

    async fn import_federation(
        &self,
        source: cdk::wallet::FederationSource,
        local_mint_url: Option<cdk::mint_url::MintUrl>,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        let federation = self
            .federations
            .import_with_transport(
                source,
                cdk::wallet::FederationImportOptions {
                    local_mint_url,
                    connector_policy: None,
                },
                &FederationTransport::default(),
            )
            .await?;
        self.activate_federation(&federation, target_proof_count)
            .await
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

    #[cfg(feature = "federation")]
    fn federation_config() -> cdk::federation::FederationConfig {
        use std::collections::BTreeMap;
        use std::str::FromStr;

        use cdk::nuts::nut01::BlsSecretKey;
        use cdk_common::federation::{
            generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
            TrustedDealerMember,
        };

        let member = |id: u16| {
            let mut secret = [0u8; 32];
            secret[31] = id as u8;
            TrustedDealerMember::new(
                MemberId::new(id).expect("member id"),
                cdk::mint_url::MintUrl::from_str(&format!("https://ffi-mint{id}.example.com"))
                    .expect("public mint url"),
                cdk::mint_url::MintUrl::from_str(&format!("https://ffi-fed{id}.example.com"))
                    .expect("federation api url"),
                cdk::nuts::SecretKey::from_slice(&secret).expect("identity secret"),
            )
        };
        let key = |byte: u8| BlsSecretKey::from_reduced_bytes(&[byte; 32]);
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("params"),
            vec![member(1), member(2), member(3)],
            vec![TrustedDealerKeysetSpec::new(
                cdk::nuts::CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([(cdk::Amount::from(1), vec![key(1), key(2)])]),
            )],
        )
        .expect("trusted dealer configs")
        .public_config
    }

    #[cfg(feature = "federation")]
    #[tokio::test(flavor = "multi_thread")]
    async fn imported_federation_is_reactivated_after_restart() {
        let config = federation_config();
        let local_mint_url =
            MintUrl::new("https://ffi-mint1.example.com".to_string()).expect("local mint url");
        let member_two_url =
            MintUrl::new("https://ffi-mint2.example.com".to_string()).expect("member url");
        let db = WalletSqliteDatabase::new_in_memory().expect("in-memory wallet db should open");

        let repo = WalletRepository::new(MNEMONIC.to_string(), custom_wallet_store(db.clone()))
            .expect("repository should be created");
        let info = repo
            .add_federation_config(
                FederationWalletConfig {
                    local_mint_url: local_mint_url.clone(),
                    public_config_json: serde_json::to_string(&config).expect("config json"),
                },
                None,
            )
            .await
            .expect("import federation");
        assert_eq!(info.federation_id, config.federation_id.to_string());
        drop(repo);

        let restarted = WalletRepository::new(MNEMONIC.to_string(), custom_wallet_store(db))
            .expect("repository should be recreated");
        let federations = restarted.list_federations().await.expect("list");
        assert_eq!(federations.len(), 1);
        assert_eq!(federations[0].federation_id, info.federation_id);
        assert_eq!(
            federations[0].wallet_config.local_mint_url,
            local_mint_url.clone()
        );

        // A member URL resolves to the federation's local wallet without re-importing.
        let wallet = restarted
            .get_wallet(member_two_url, CurrencyUnit::Sat)
            .await
            .expect("federation wallet reactivated");
        assert_eq!(wallet.mint_url(), local_mint_url);
    }

    #[cfg(feature = "federation")]
    async fn repository_with_federation(
        db: Arc<WalletSqliteDatabase>,
    ) -> (WalletRepository, FederationWalletInfo) {
        let repo = WalletRepository::new(MNEMONIC.to_string(), custom_wallet_store(db))
            .expect("repository should be created");
        let info = repo
            .add_federation_config(
                FederationWalletConfig {
                    local_mint_url: MintUrl::new("https://ffi-mint1.example.com".to_string())
                        .expect("local mint url"),
                    public_config_json: serde_json::to_string(&federation_config())
                        .expect("config json"),
                },
                None,
            )
            .await
            .expect("import federation");
        (repo, info)
    }

    #[cfg(feature = "federation")]
    #[tokio::test(flavor = "multi_thread")]
    async fn federation_wallet_accepts_every_member_url() {
        let db = WalletSqliteDatabase::new_in_memory().expect("in-memory wallet db should open");
        let (repo, info) = repository_with_federation(db).await;
        let local_mint_url = info.wallet_config.local_mint_url.clone();
        let member_two_url =
            MintUrl::new("https://ffi-mint2.example.com".to_string()).expect("member url");

        let wallet = repo
            .get_or_create_wallet(member_two_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("member url resolves to the federation wallet");
        assert_eq!(wallet.mint_url(), local_mint_url);
        assert_eq!(wallet.mint_url_aliases().len(), 2);
        assert!(!wallet.mint_url_aliases().contains(&local_mint_url));
        for member in &info.member_mint_urls {
            assert!(wallet.accepts_mint_url(member.clone()));
        }
        assert!(!wallet.accepts_mint_url(
            MintUrl::new("https://unrelated.example.com".to_string()).expect("url")
        ));
        assert_eq!(repo.get_wallets().await.len(), 1, "no member-keyed wallet");
        assert!(repo.has_mint(member_two_url).await);
    }

    #[cfg(feature = "federation")]
    #[tokio::test(flavor = "multi_thread")]
    async fn removed_federation_is_not_reactivated_after_restart() {
        let db = WalletSqliteDatabase::new_in_memory().expect("in-memory wallet db should open");
        let (repo, info) = repository_with_federation(db.clone()).await;
        let member_two_url =
            MintUrl::new("https://ffi-mint2.example.com".to_string()).expect("member url");
        assert!(repo.has_mint(member_two_url.clone()).await);

        assert!(repo
            .remove_federation(info.federation_id.clone())
            .await
            .expect("remove federation"));
        assert!(!repo
            .remove_federation(info.federation_id.clone())
            .await
            .expect("second removal is a no-op"));
        assert!(repo.list_federations().await.expect("list").is_empty());
        assert!(repo.get_wallets().await.is_empty());
        assert!(!repo.has_mint(member_two_url.clone()).await);
        assert!(
            !repo
                .has_mint(info.wallet_config.local_mint_url.clone())
                .await
        );
        drop(repo);

        let restarted = WalletRepository::new(MNEMONIC.to_string(), custom_wallet_store(db))
            .expect("repository should be recreated");
        assert!(restarted.list_federations().await.expect("list").is_empty());
        assert!(restarted
            .get_wallet(member_two_url.clone(), CurrencyUnit::Sat)
            .await
            .is_err());
        assert!(!restarted.has_mint(member_two_url.clone()).await);

        // Without the federation a member URL is an ordinary mint URL again.
        let standalone = restarted
            .get_or_create_wallet(member_two_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("standalone wallet");
        assert_eq!(standalone.mint_url(), member_two_url);
        assert!(standalone.mint_url_aliases().is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test(flavor = "multi_thread")]
    async fn remove_federation_rejects_an_invalid_id() {
        let repo = test_repository();
        assert!(repo.remove_federation("not hex".to_string()).await.is_err());
    }
}

#[cfg(feature = "federation")]
#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
    /// Import and activate a federation from an invite code.
    ///
    /// Imported federations are stored in the wallet database and reactivated
    /// when the repository is constructed again. Importing a federation that
    /// is already imported refreshes its config and keeps its local mint URL.
    pub async fn add_federation_invite(
        &self,
        invite_code: String,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        self.import_federation(
            cdk::wallet::FederationSource::Invite(invite_code),
            None,
            target_proof_count,
        )
        .await
    }

    /// Discover, import, and activate a federation through any member mint URL.
    pub async fn add_federation_mint_url(
        &self,
        mint_url: MintUrl,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        self.import_federation(
            cdk::wallet::FederationSource::MemberUrl(mint_url.try_into()?),
            None,
            target_proof_count,
        )
        .await
    }

    /// Import and activate a public federation config under a local mint URL.
    pub async fn add_federation_config(
        &self,
        federation_config: FederationWalletConfig,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        let local_mint_url: cdk::mint_url::MintUrl =
            federation_config.local_mint_url.clone().try_into()?;
        let public_config = federation_config.public_config()?;
        self.import_federation(
            cdk::wallet::FederationSource::PublicConfig(Box::new(public_config)),
            Some(local_mint_url),
            target_proof_count,
        )
        .await
    }

    /// List the federations imported into this wallet.
    pub async fn list_federations(&self) -> Result<Vec<FederationWalletInfo>, FfiError> {
        self.federations
            .list()
            .await?
            .iter()
            .map(FederationWalletInfo::try_from)
            .collect()
    }

    /// Remove an imported federation.
    ///
    /// Deletes the federation from the wallet's federation registry, so it is
    /// not reactivated when the repository is constructed again, and removes
    /// its wallets (every unit under its local mint URL) from this repository.
    /// Its member URLs no longer resolve to any wallet. Returns false when no
    /// federation with this ID is imported.
    ///
    /// Like `remove_wallet`, this never deletes funds: the federation's
    /// proofs, quotes and transactions stay in the database under its local
    /// mint URL. Importing the federation again under the same local mint URL
    /// brings its balance back. A mint record that the wallet stored for the
    /// local mint URL also stays, so a later repository construction loads it
    /// as an ordinary, non-federated wallet, as it does for any mint removed
    /// with `remove_wallet`.
    pub async fn remove_federation(&self, federation_id: String) -> Result<bool, FfiError> {
        let federation_id = cdk::federation::FederationId::from_hex(&federation_id)?;
        let Some(federation) = self.federations.get(&federation_id).await? else {
            return Ok(false);
        };
        self.federations.remove(federation_id).await?;
        self.inner
            .remove_wallets_for_mint(federation.local_mint_url())
            .await;
        Ok(true)
    }

    /// Rediscover an imported federation through its members and store the
    /// refreshed config.
    pub async fn refresh_federation(
        &self,
        federation_id: String,
        target_proof_count: Option<u32>,
    ) -> Result<FederationWalletInfo, FfiError> {
        let federation_id = cdk::federation::FederationId::from_hex(&federation_id)?;
        let federation = self
            .federations
            .refresh_with_transport(federation_id, &FederationTransport::default())
            .await?;
        self.activate_federation(&federation, target_proof_count)
            .await
    }
}
