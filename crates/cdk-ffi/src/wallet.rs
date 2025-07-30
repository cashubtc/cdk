//! FFI Wallet bindings

use std::sync::Arc;

use cdk::wallet::{Wallet as CdkWallet, WalletBuilder as CdkWalletBuilder};
use cdk_sqlite::wallet::memory;
use tokio::runtime::Runtime;

use crate::error::FfiError;
use crate::types::*;

/// FFI-compatible Wallet
#[derive(uniffi::Object)]
pub struct Wallet {
    inner: Arc<CdkWallet>,
    runtime: Arc<Runtime>,
}

#[uniffi::export]
impl Wallet {
    /// Create a new Wallet
    #[uniffi::constructor]
    pub fn new(
        mint_url: String,
        unit: CurrencyUnit,
        seed: Vec<u8>,
        target_proof_count: Option<u32>,
    ) -> Result<Self, FfiError> {
        let runtime =
            Arc::new(Runtime::new().map_err(|e| FfiError::Generic { msg: e.to_string() })?);

        let inner = runtime.block_on(async {
            let localstore = memory::empty()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;

            let wallet =
                CdkWalletBuilder::new()
                    .mint_url(mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                        FfiError::Generic { msg: e.to_string() }
                    })?)
                    .unit(unit.into())
                    .localstore(Arc::new(localstore))
                    .seed(&seed)
                    .target_proof_count(target_proof_count.unwrap_or(3) as usize)
                    .build()
                    .map_err(FfiError::from)?;

            Ok::<_, FfiError>(Arc::new(wallet))
        })?;

        Ok(Self { inner, runtime })
    }

    /// Get the mint URL
    pub fn mint_url(&self) -> MintUrl {
        self.inner.mint_url.clone().into()
    }

    /// Get the currency unit
    pub fn unit(&self) -> CurrencyUnit {
        self.inner.unit.clone().into()
    }

    /// Get total balance
    pub fn total_balance(&self) -> Result<Amount, FfiError> {
        self.runtime.block_on(async {
            let balance = self.inner.total_balance().await?;
            Ok(balance.into())
        })
    }

    /// Get total pending balance
    pub fn total_pending_balance(&self) -> Result<Amount, FfiError> {
        self.runtime.block_on(async {
            let balance = self.inner.total_pending_balance().await?;
            Ok(balance.into())
        })
    }

    /// Get total reserved balance  
    pub fn total_reserved_balance(&self) -> Result<Amount, FfiError> {
        self.runtime.block_on(async {
            let balance = self.inner.total_reserved_balance().await?;
            Ok(balance.into())
        })
    }

    /// Get mint info
    pub fn get_mint_info(&self) -> Result<Option<String>, FfiError> {
        self.runtime.block_on(async {
            let info = self.inner.get_mint_info().await?;
            Ok(info.map(|i| serde_json::to_string(&i).unwrap_or_default()))
        })
    }

    /// Send tokens directly (simplified API)
    pub fn send(
        &self,
        amount: Amount,
        options: SendOptions,
        memo: Option<String>,
    ) -> Result<Token, FfiError> {
        self.runtime.block_on(async {
            let prepared = self
                .inner
                .prepare_send(amount.into(), options.into())
                .await?;

            let send_memo = memo.map(|m| cdk::wallet::SendMemo::for_token(&m));
            let token = self.inner.send(prepared, send_memo).await?;

            Ok(token.into())
        })
    }

    /// Receive tokens
    pub fn receive(&self, token: Token, options: ReceiveOptions) -> Result<Amount, FfiError> {
        self.runtime.block_on(async {
            let amount = self.inner.receive(&token.token, options.into()).await?;
            Ok(amount.into())
        })
    }

    /// Restore wallet from seed
    pub fn restore(&self) -> Result<Amount, FfiError> {
        self.runtime.block_on(async {
            let amount = self.inner.restore().await?;
            Ok(amount.into())
        })
    }

    /// Verify token DLEQ proofs
    pub fn verify_token_dleq(&self, token: Token) -> Result<(), FfiError> {
        self.runtime.block_on(async {
            let cdk_token: cdk::nuts::Token = token.try_into()?;
            self.inner.verify_token_dleq(&cdk_token).await?;
            Ok(())
        })
    }
}

/// Builder configuration for creating wallets
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletConfig {
    pub mint_url: Option<String>,
    pub unit: Option<CurrencyUnit>,
    pub seed: Option<Vec<u8>>,
    pub target_proof_count: Option<u32>,
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            mint_url: None,
            unit: None,
            seed: None,
            target_proof_count: Some(3),
        }
    }
}

/// Create a wallet builder for more advanced configurations
#[derive(uniffi::Object)]
pub struct WalletBuilder {
    config: WalletConfig,
    runtime: Arc<Runtime>,
}

#[uniffi::export]
impl WalletBuilder {
    /// Create a new WalletBuilder
    #[uniffi::constructor]
    pub fn new() -> Result<Self, FfiError> {
        let runtime =
            Arc::new(Runtime::new().map_err(|e| FfiError::Generic { msg: e.to_string() })?);

        Ok(Self {
            config: WalletConfig::default(),
            runtime,
        })
    }

    /// Set mint URL
    pub fn mint_url(&self, mint_url: String) -> Result<Self, FfiError> {
        // Validate URL
        let _parsed = mint_url
            .parse::<cdk::mint_url::MintUrl>()
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;

        let mut config = self.config.clone();
        config.mint_url = Some(mint_url);

        Ok(Self {
            config,
            runtime: self.runtime.clone(),
        })
    }

    /// Set currency unit
    pub fn unit(&self, unit: CurrencyUnit) -> Self {
        let mut config = self.config.clone();
        config.unit = Some(unit);

        Self {
            config,
            runtime: self.runtime.clone(),
        }
    }

    /// Set seed
    pub fn seed(&self, seed: Vec<u8>) -> Self {
        let mut config = self.config.clone();
        config.seed = Some(seed);

        Self {
            config,
            runtime: self.runtime.clone(),
        }
    }

    /// Set target proof count
    pub fn target_proof_count(&self, count: u32) -> Self {
        let mut config = self.config.clone();
        config.target_proof_count = Some(count);

        Self {
            config,
            runtime: self.runtime.clone(),
        }
    }

    /// Build the wallet
    pub fn build(&self) -> Result<Wallet, FfiError> {
        let mint_url = self
            .config
            .mint_url
            .as_ref()
            .ok_or_else(|| FfiError::Generic {
                msg: "mint_url is required".to_string(),
            })?;
        let unit = self.config.unit.as_ref().ok_or_else(|| FfiError::Generic {
            msg: "unit is required".to_string(),
        })?;
        let seed = self.config.seed.as_ref().ok_or_else(|| FfiError::Generic {
            msg: "seed is required".to_string(),
        })?;

        self.runtime.block_on(async {
            let localstore = memory::empty()
                .await
                .map_err(|e| FfiError::Database { msg: e.to_string() })?;

            let wallet =
                CdkWalletBuilder::new()
                    .mint_url(mint_url.parse().map_err(|e: cdk::mint_url::Error| {
                        FfiError::Generic { msg: e.to_string() }
                    })?)
                    .unit(unit.clone().into())
                    .localstore(Arc::new(localstore))
                    .seed(seed)
                    .target_proof_count(self.config.target_proof_count.unwrap_or(3) as usize)
                    .build()
                    .map_err(FfiError::from)?;

            Ok(Wallet {
                inner: Arc::new(wallet),
                runtime: self.runtime.clone(),
            })
        })
    }
}

/// Utility functions
#[uniffi::export]
pub fn generate_seed() -> Vec<u8> {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::rng().fill_bytes(&mut seed);
    seed.to_vec()
}
