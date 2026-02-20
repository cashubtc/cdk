//! FFI WalletRepository bindings

use std::collections::HashMap;
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::wallet_repository::{
    WalletRepository as CdkWalletRepository, WalletRepositoryBuilder,
};

use crate::error::FfiError;
use crate::types::*;

/// FFI-compatible WalletRepository
#[derive(uniffi::Object)]
pub struct WalletRepository {
    inner: Arc<CdkWalletRepository>,
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletRepository {
    /// Create a new WalletRepository from mnemonic using WalletDatabaseFfi trait
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new(
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move {
                    WalletRepositoryBuilder::new()
                        .localstore(localstore)
                        .seed(seed)
                        .build()
                        .await
                })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move {
                        WalletRepositoryBuilder::new()
                            .localstore(localstore)
                            .seed(seed)
                            .build()
                            .await
                    })
            }
        }?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Create a new WalletRepository with proxy configuration
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new_with_proxy(
        mnemonic: String,
        db: Arc<dyn crate::database::WalletDatabase>,
        proxy_url: String,
    ) -> Result<Self, FfiError> {
        // Parse mnemonic and generate seed without passphrase
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| FfiError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // Convert the FFI database trait to a CDK database implementation
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        // Parse proxy URL
        let proxy_url = url::Url::parse(&proxy_url)
            .map_err(|e| FfiError::internal(format!("Invalid URL: {}", e)))?;

        let wallet = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                handle.block_on(async move {
                    WalletRepositoryBuilder::new()
                        .localstore(localstore)
                        .seed(seed)
                        .proxy_url(proxy_url)
                        .build()
                        .await
                })
            }),
            Err(_) => {
                // No current runtime, create a new one
                tokio::runtime::Runtime::new()
                    .map_err(|e| FfiError::internal(format!("Failed to create runtime: {}", e)))?
                    .block_on(async move {
                        WalletRepositoryBuilder::new()
                            .localstore(localstore)
                            .seed(seed)
                            .proxy_url(proxy_url)
                            .build()
                            .await
                    })
            }
        }?;

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

    /// Check if mint is in wallet
    pub async fn has_mint(&self, mint_url: MintUrl) -> bool {
        if let Ok(cdk_mint_url) = mint_url.try_into() {
            self.inner.has_mint(&cdk_mint_url).await
        } else {
            false
        }
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
}

/// Token data FFI type
///
/// Contains information extracted from a parsed token.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
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
