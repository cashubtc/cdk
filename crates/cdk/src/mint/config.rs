//! Active mint configuration
//!
//! This is the active configuration that can be updated at runtime.
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{Id, MintInfo, MintKeySet};
use crate::mint_url::MintUrl;

/// Mint Inner configuration
pub struct Inner {
    /// Active Mint Keysets
    pub keysets: HashMap<Id, MintKeySet>,
    /// Mint url
    pub mint_info: MintInfo,
    /// Mint config
    pub mint_url: MintUrl,
}

/// Mint configuration
///
/// This struct is used to configure the mint, and it is wrapped inside a ArcSwap, so it can be
/// updated at runtime without locking the shared config nor without requiriming a mutable reference
/// to the config
///
/// ArcSwap is used instead of a RwLock since the updates should be less frequent than the reads
#[derive(Clone)]
pub struct Config {
    inner: Arc<ArcSwap<Inner>>,
}

impl Config {
    /// Creates a new configuration instance
    pub fn new(mint_url: MintUrl, mint_info: MintInfo, keysets: HashMap<Id, MintKeySet>) -> Self {
        let inner = Inner {
            keysets,
            mint_info,
            mint_url,
        };

        Self {
            inner: Arc::new(ArcSwap::from_pointee(inner)),
        }
    }

    /// Gets an Arc of the current configuration
    pub fn get_config(&self) -> Arc<Inner> {
        self.inner.load().clone()
    }

    /// Gets a copy of the mint url
    pub fn mint_url(&self) -> MintUrl {
        self.get_config().mint_url.clone()
    }

    /// Replace the current mint url with a new one
    pub fn set_mint_url(&self, mint_url: MintUrl) {
        let current_inner = self.get_config();
        let new_inner = Inner {
            mint_url,
            mint_info: current_inner.mint_info.clone(),
            keysets: current_inner.keysets.clone(),
        };

        self.inner.store(Arc::new(new_inner));
    }

    /// Gets a copy of the mint info
    pub fn mint_info(&self) -> MintInfo {
        self.get_config().mint_info.clone()
    }

    /// Replaces the current mint info with a new one
    pub fn set_mint_info(&self, mint_info: MintInfo) {
        let current_inner = self.get_config();
        let new_inner = Inner {
            mint_info,
            mint_url: current_inner.mint_url.clone(),
            keysets: current_inner.keysets.clone(),
        };

        self.inner.store(Arc::new(new_inner));
    }

    /// Replaces the current keysets with a new one
    pub fn set_keysets(&self, keysets: HashMap<Id, MintKeySet>) {
        let current_inner = self.get_config();
        let new_inner = Inner {
            mint_info: current_inner.mint_info.clone(),
            mint_url: current_inner.mint_url.clone(),
            keysets,
        };

        self.inner.store(Arc::new(new_inner));
    }
}
