//! Lightning Network common environment variables

use std::env;

use crate::config::Ln;

// LN environment variables
pub const ENV_LN_BACKEND: &str = "CDK_MINTD_LN_BACKEND";
pub const ENV_LN_INVOICE_DESCRIPTION: &str = "CDK_MINTD_LN_INVOICE_DESCRIPTION";
pub const ENV_LN_MIN_MINT: &str = "CDK_MINTD_LN_MIN_MINT";
pub const ENV_LN_MAX_MINT: &str = "CDK_MINTD_LN_MAX_MINT";
pub const ENV_LN_MIN_MELT: &str = "CDK_MINTD_LN_MIN_MELT";
pub const ENV_LN_MAX_MELT: &str = "CDK_MINTD_LN_MAX_MELT";

impl Ln {
    pub fn from_env(mut self) -> Self {
        // LnBackend
        if let Ok(backend_str) = env::var(ENV_LN_BACKEND) {
            if let Ok(backend) = backend_str.parse() {
                self.ln_backend = backend;
            } else {
                tracing::warn!("Unknow payment backend set in env var will attempt to use config file. {backend_str}");
            }
        }

        // Optional invoice description
        if let Ok(description) = env::var(ENV_LN_INVOICE_DESCRIPTION) {
            self.invoice_description = Some(description);
        }

        // Amount fields
        if let Ok(min_mint_str) = env::var(ENV_LN_MIN_MINT) {
            if let Ok(amount) = min_mint_str.parse::<u64>() {
                self.min_mint = amount.into();
            }
        }

        if let Ok(max_mint_str) = env::var(ENV_LN_MAX_MINT) {
            if let Ok(amount) = max_mint_str.parse::<u64>() {
                self.max_mint = amount.into();
            }
        }

        if let Ok(min_melt_str) = env::var(ENV_LN_MIN_MELT) {
            if let Ok(amount) = min_melt_str.parse::<u64>() {
                self.min_melt = amount.into();
            }
        }

        if let Ok(max_melt_str) = env::var(ENV_LN_MAX_MELT) {
            if let Ok(amount) = max_melt_str.parse::<u64>() {
                self.max_melt = amount.into();
            }
        }

        self
    }
}
