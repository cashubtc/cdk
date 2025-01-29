//! Active mint configuration
//!
//! This is the active configuration that can be updated at runtime.
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{Id, MintKeySet};
use crate::types::QuoteTTL;

/// Mint Inner configuration
pub struct Config {
    /// Active Mint Keysets
    pub keysets: HashMap<Id, MintKeySet>,
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
    pub fn new(quote_ttl: QuoteTTL, keysets: HashMap<Id, MintKeySet>) -> Self {
        let inner = Config { keysets, quote_ttl };

        Self {
            config: Arc::new(ArcSwap::from_pointee(inner)),
        }
    }

    /// Gets an Arc of the current configuration
    pub fn load(&self) -> Arc<Config> {
        self.config.load().clone()
    }

    /// Gets a copy of the quote ttl
    pub fn quote_ttl(&self) -> QuoteTTL {
        self.load().quote_ttl
    }

    /// Replaces the current quote ttl with a new one
    pub fn set_quote_ttl(&self, quote_ttl: QuoteTTL) {
        let current_inner = self.load();
        let new_inner = Config {
            quote_ttl,
            keysets: current_inner.keysets.clone(),
        };

        self.config.store(Arc::new(new_inner));
    }

    /// Replaces the current keysets with a new one
    pub fn set_keysets(&self, keysets: HashMap<Id, MintKeySet>) {
        let current_inner = self.load();
        let new_inner = Config {
            quote_ttl: current_inner.quote_ttl,
            keysets,
        };

        self.config.store(Arc::new(new_inner));
    }
}
