//! Onchain environment variables

use std::env;

use crate::config::Onchain;

// Onchain environment variables
pub const ENV_ONCHAIN_BACKEND: &str = "CDK_MINTD_ONCHAIN_BACKEND";
pub const ENV_ONCHAIN_MIN_MINT: &str = "CDK_MINTD_ONCHAIN_MIN_MINT";
pub const ENV_ONCHAIN_MAX_MINT: &str = "CDK_MINTD_ONCHAIN_MAX_MINT";
pub const ENV_ONCHAIN_MIN_MELT: &str = "CDK_MINTD_ONCHAIN_MIN_MELT";
pub const ENV_ONCHAIN_MAX_MELT: &str = "CDK_MINTD_ONCHAIN_MAX_MELT";

impl Onchain {
    pub fn from_env(mut self) -> Self {
        // OnchainBackend
        if let Ok(backend_str) = env::var(ENV_ONCHAIN_BACKEND) {
            if let Ok(backend) = backend_str.parse() {
                self.onchain_backend = backend;
            } else {
                tracing::warn!("Unknown onchain backend set in env var will attempt to use config file. {backend_str}");
            }
        }

        // Amount fields
        if let Ok(min_mint_str) = env::var(ENV_ONCHAIN_MIN_MINT) {
            if let Ok(amount) = min_mint_str.parse::<u64>() {
                self.min_mint = amount.into();
            }
        }

        if let Ok(max_mint_str) = env::var(ENV_ONCHAIN_MAX_MINT) {
            if let Ok(amount) = max_mint_str.parse::<u64>() {
                self.max_mint = amount.into();
            }
        }

        if let Ok(min_melt_str) = env::var(ENV_ONCHAIN_MIN_MELT) {
            if let Ok(amount) = min_melt_str.parse::<u64>() {
                self.min_melt = amount.into();
            }
        }

        if let Ok(max_melt_str) = env::var(ENV_ONCHAIN_MAX_MELT) {
            if let Ok(amount) = max_melt_str.parse::<u64>() {
                self.max_melt = amount.into();
            }
        }

        self
    }
}
