//! WASM WalletRepository bindings

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::wallet_repository::{
    WalletRepository as CdkWalletRepository, WalletRepositoryBuilder,
};
use wasm_bindgen::prelude::*;

use crate::error::WasmError;
use crate::types::*;

/// WASM-compatible WalletRepository
#[wasm_bindgen]
#[derive(Debug)]
pub struct WalletRepository {
    inner: Arc<CdkWalletRepository>,
}

#[wasm_bindgen]
impl WalletRepository {
    /// Create a new WalletRepository from mnemonic
    pub async fn new(mnemonic: String, db: JsValue) -> Result<WalletRepository, WasmError> {
        let m = Mnemonic::parse(&mnemonic)
            .map_err(|e| WasmError::internal(format!("Invalid mnemonic: {}", e)))?;
        let seed = m.to_seed_normalized("");

        // TODO: Accept a JS database implementation
        let _ = db;
        let localstore = crate::local_storage::LocalStorageDatabase::new().into_arc();

        let wallet = WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed(seed)
            .build()
            .await?;

        Ok(Self {
            inner: Arc::new(wallet),
        })
    }

    /// Set metadata cache TTL in seconds for a specific mint
    #[wasm_bindgen(js_name = "setMetadataCacheTtlForMint")]
    pub async fn set_metadata_cache_ttl_for_mint(
        &self,
        mint_url: String,
        ttl_secs: Option<u64>,
    ) -> Result<(), WasmError> {
        let cdk_mint_url: cdk::mint_url::MintUrl =
            mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                WasmError::internal(format!("Invalid URL: {}", e))
            })?;
        let wallets = self.inner.get_wallets().await;

        if let Some(wallet) = wallets.iter().find(|w| w.mint_url == cdk_mint_url) {
            let ttl = ttl_secs.map(std::time::Duration::from_secs);
            wallet.set_metadata_cache_ttl(ttl);
            Ok(())
        } else {
            Err(WasmError::internal(format!(
                "Mint not found: {}",
                cdk_mint_url
            )))
        }
    }

    /// Set metadata cache TTL in seconds for all mints
    #[wasm_bindgen(js_name = "setMetadataCacheTtlForAllMints")]
    pub async fn set_metadata_cache_ttl_for_all_mints(&self, ttl_secs: Option<u64>) {
        let wallets = self.inner.get_wallets().await;
        let ttl = ttl_secs.map(std::time::Duration::from_secs);

        for wallet in wallets.iter() {
            wallet.set_metadata_cache_ttl(ttl);
        }
    }

    /// Add a mint to this WalletRepository
    #[wasm_bindgen(js_name = "addMint")]
    pub async fn add_mint(
        &self,
        mint_url: String,
        unit: JsValue,
        target_proof_count: Option<u32>,
    ) -> Result<(), WasmError> {
        let cdk_mint_url: cdk::mint_url::MintUrl =
            mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                WasmError::internal(format!("Invalid URL: {}", e))
            })?;

        let config = target_proof_count.map(|count| {
            cdk::wallet::wallet_repository::WalletConfig::new()
                .with_target_proof_count(count as usize)
        });

        let unit_enum: CurrencyUnit = if unit.is_null() || unit.is_undefined() {
            CurrencyUnit::Sat
        } else {
            serde_wasm_bindgen::from_value(unit).map_err(WasmError::internal)?
        };

        self.inner
            .create_wallet(cdk_mint_url, unit_enum.into(), config)
            .await?;

        Ok(())
    }

    /// Remove mint from WalletRepository
    #[wasm_bindgen(js_name = "removeMint")]
    pub async fn remove_mint(
        &self,
        mint_url: String,
        currency_unit: JsValue,
    ) -> Result<(), WasmError> {
        let cdk_mint_url: cdk::mint_url::MintUrl =
            mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                WasmError::internal(format!("Invalid URL: {}", e))
            })?;

        let unit: CurrencyUnit =
            serde_wasm_bindgen::from_value(currency_unit).map_err(WasmError::internal)?;

        self.inner
            .remove_wallet(cdk_mint_url, unit.into())
            .await
            .map_err(|e| e.into())
    }

    /// Check if mint is in wallet
    #[wasm_bindgen(js_name = "hasMint")]
    pub async fn has_mint(&self, mint_url: String) -> bool {
        if let Ok(cdk_mint_url) = mint_url.parse::<cdk::mint_url::MintUrl>() {
            self.inner.has_mint(&cdk_mint_url).await
        } else {
            false
        }
    }

    /// Get wallet balances for all mints
    #[wasm_bindgen(js_name = "getBalances")]
    pub async fn get_balances(&self) -> Result<JsValue, WasmError> {
        let balances = self.inner.get_balances().await?;
        let balance_map: std::collections::HashMap<String, Amount> = balances
            .into_iter()
            .map(|(mint_url, amount)| (mint_url.to_string(), amount.into()))
            .collect();
        serde_wasm_bindgen::to_value(&balance_map).map_err(WasmError::internal)
    }

    /// Get all wallets from WalletRepository
    #[wasm_bindgen(js_name = "getWallets")]
    pub async fn get_wallets(&self) -> Vec<crate::wallet::Wallet> {
        let wallets = self.inner.get_wallets().await;
        wallets
            .into_iter()
            .map(|w| crate::wallet::Wallet::from_inner(Arc::new(w)))
            .collect()
    }

    /// Get a specific wallet by mint URL and unit
    #[wasm_bindgen(js_name = "getWallet")]
    pub async fn get_wallet(
        &self,
        mint_url: String,
        unit: JsValue,
    ) -> Result<crate::wallet::Wallet, WasmError> {
        let cdk_mint_url: cdk::mint_url::MintUrl =
            mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                WasmError::internal(format!("Invalid URL: {}", e))
            })?;
        let unit_enum: CurrencyUnit =
            serde_wasm_bindgen::from_value(unit).map_err(WasmError::internal)?;
        let unit_cdk: cdk::nuts::CurrencyUnit = unit_enum.into();
        let wallet = self.inner.get_wallet(&cdk_mint_url, &unit_cdk).await?;
        Ok(crate::wallet::Wallet::from_inner(Arc::new(wallet)))
    }
}

/// Token data type for WASM
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenData {
    pub mint_url: MintUrl,
    pub proofs: Proofs,
    pub memo: Option<String>,
    pub value: Amount,
    pub unit: CurrencyUnit,
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
