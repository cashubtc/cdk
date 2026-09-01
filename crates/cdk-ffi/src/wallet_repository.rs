//! Internal construction of the engine's multi-mint wallet repository.

use std::sync::Arc;

use bip39::Mnemonic;
use cdk::wallet::wallet_repository::{
    WalletRepository as CdkWalletRepository, WalletRepositoryBuilder,
};

use crate::error::FfiError;
use crate::wallet::RateLimit;

/// Configuration used by the portable multi-mint facade.
#[derive(Debug, Clone)]
pub(crate) struct WalletRepositoryConfig {
    /// Proxy used by every mint operation. Omit for a direct connection.
    pub(crate) proxy_url: Option<String>,
    /// Shared request pacing. Omit to use the built-in default.
    pub(crate) rate_limit: Option<RateLimit>,
}

/// Internal bridge to the engine repository.
pub(crate) struct WalletRepository {
    inner: Arc<CdkWalletRepository>,
}

impl WalletRepository {
    pub(crate) fn inner(&self) -> Arc<CdkWalletRepository> {
        Arc::clone(&self.inner)
    }

    /// Restore locally configured wallets without contacting their mints.
    pub(crate) fn new_with_config(
        mnemonic: String,
        store: crate::database::WalletStore,
        config: WalletRepositoryConfig,
    ) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(store)?;
        let mnemonic = Mnemonic::parse(&mnemonic)
            .map_err(|error| FfiError::invalid_input(format!("Invalid mnemonic: {error}")))?;
        let seed = mnemonic.to_seed_normalized("");
        let localstore = crate::database::create_cdk_database_from_ffi(db);

        let proxy_url = config
            .proxy_url
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|error| FfiError::invalid_input(format!("Invalid URL: {error}")))?;
        let rate_limit = config
            .rate_limit
            .as_ref()
            .map(RateLimit::to_config)
            .transpose()?;

        let runtime = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let repository = runtime.block_on(async move {
            let mut builder = WalletRepositoryBuilder::new()
                .localstore(localstore)
                .seed(seed);
            if let Some(proxy_url) = proxy_url {
                builder = builder.proxy_url(proxy_url);
            }
            builder = match rate_limit {
                Some(Some(rate_limit)) => builder.with_rate_limiting_config(rate_limit),
                Some(None) => builder.with_rate_limiting_disabled(),
                None => builder,
            };
            builder.build().await
        })?;

        Ok(Self {
            inner: Arc::new(repository),
        })
    }
}
