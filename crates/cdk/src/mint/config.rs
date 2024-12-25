//! Active mint configuration
//!
//! This is the active configuration that can be updated at runtime.
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{Id, MintInfo, MintKeySet};
use crate::mint_url::MintUrl;
use crate::types::QuoteTTL;

/// Mint Inner configuration
pub struct Config {
    /// Active Mint Keysets
    pub keysets: HashMap<Id, MintKeySet>,
    /// Mint url
    pub mint_info: MintInfo,
    /// Mint config
    pub mint_url: MintUrl,
    /// Quotes ttl
    pub quote_ttl: QuoteTTL,
}

/// Mint configuration
///
/// This struct is used to configure the mint, and it is wrapped inside a ArcSwap, so it can be
/// updated at runtime without locking the shared config nor without requiriming a mutable reference
/// to the config
///
/// ArcSwap is used instead of a RwLock since the updates should be less frequent than the reads
#[derive(Clone)]
pub struct SwappableConfig {
    config: Arc<ArcSwap<Config>>,
}

impl SwappableConfig {
    /// Creates a new configuration instance
    pub fn new(
        mint_url: MintUrl,
        quote_ttl: QuoteTTL,
        mint_info: MintInfo,
        keysets: HashMap<Id, MintKeySet>,
    ) -> Self {
        let inner = Config {
            keysets,
            quote_ttl,
            mint_info,
            mint_url,
        };

        Self {
            config: Arc::new(ArcSwap::from_pointee(inner)),
        }
    }

    /// Gets an Arc of the current configuration
    pub fn load(&self) -> Arc<Config> {
        self.config.load().clone()
    }

    /// Gets a copy of the mint url
    pub fn mint_url(&self) -> MintUrl {
        self.load().mint_url.clone()
    }

    /// Replace the current mint url with a new one
    pub fn set_mint_url(&self, mint_url: MintUrl) {
        let current_inner = self.load();
        let new_inner = Config {
            mint_url,
            quote_ttl: current_inner.quote_ttl,
            mint_info: current_inner.mint_info.clone(),
            keysets: current_inner.keysets.clone(),
        };

        self.config.store(Arc::new(new_inner));
    }

    /// Gets a copy of the quote ttl
    pub fn quote_ttl(&self) -> QuoteTTL {
        self.load().quote_ttl
    }

    /// Replaces the current quote ttl with a new one
    pub fn set_quote_ttl(&self, quote_ttl: QuoteTTL) {
        let current_inner = self.load();
        let new_inner = Config {
            mint_info: current_inner.mint_info.clone(),
            mint_url: current_inner.mint_url.clone(),
            quote_ttl,
            keysets: current_inner.keysets.clone(),
        };

        self.config.store(Arc::new(new_inner));
    }

    /// Gets a copy of the mint info
    pub fn mint_info(&self) -> MintInfo {
        self.load().mint_info.clone()
    }

    /// Replaces the current mint info with a new one
    pub fn set_mint_info(&self, mint_info: MintInfo) {
        let current_inner = self.load();
        let new_inner = Config {
            mint_info,
            mint_url: current_inner.mint_url.clone(),
            quote_ttl: current_inner.quote_ttl,
            keysets: current_inner.keysets.clone(),
        };

        self.config.store(Arc::new(new_inner));
    }

    /// Replaces the current keysets with a new one
    pub fn set_keysets(&self, keysets: HashMap<Id, MintKeySet>) {
        let current_inner = self.load();
        let new_inner = Config {
            mint_info: current_inner.mint_info.clone(),
            quote_ttl: current_inner.quote_ttl,
            mint_url: current_inner.mint_url.clone(),
            keysets,
        };

        self.config.store(Arc::new(new_inner));
    }
}
