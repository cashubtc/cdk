//! Auth env

use std::env;

use crate::config::Auth;

pub const ENV_AUTH_OPENID_DISCOVERY: &str = "CDK_MINTD_AUTH_OPENID_DISCOVERY";
pub const ENV_AUTH_OPENID_CLIENT_ID: &str = "CDK_MINTD_AUTH_OPENID_CLIENT_ID";
pub const ENV_AUTH_MINT_MAX_BAT: &str = "CDK_MINTD_AUTH_MINT_MAX_BAT";
pub const ENV_AUTH_ENABLED_MINT: &str = "CDK_MINTD_AUTH_ENABLED_MINT";
pub const ENV_AUTH_ENABLED_MELT: &str = "CDK_MINTD_AUTH_ENABLED_MELT";
pub const ENV_AUTH_ENABLED_SWAP: &str = "CDK_MINTD_AUTH_ENABLED_SWAP";
pub const ENV_AUTH_ENABLED_CHECK_MINT_QUOTE: &str = "CDK_MINTD_AUTH_ENABLED_CHECK_MINT_QUOTE";
pub const ENV_AUTH_ENABLED_CHECK_MELT_QUOTE: &str = "CDK_MINTD_AUTH_ENABLED_CHECK_MELT_QUOTE";
pub const ENV_AUTH_ENABLED_RESTORE: &str = "CDK_MINTD_AUTH_ENABLED_RESTORE";
pub const ENV_AUTH_ENABLED_CHECK_PROOF_STATE: &str = "CDK_MINTD_AUTH_ENABLED_CHECK_PROOF_STATE";

impl Auth {
    pub fn from_env(mut self) -> Self {
        if let Ok(discovery) = env::var(ENV_AUTH_OPENID_DISCOVERY) {
            self.openid_discovery = discovery;
        }

        if let Ok(client_id) = env::var(ENV_AUTH_OPENID_CLIENT_ID) {
            self.openid_client_id = client_id;
        }

        if let Ok(max_bat_str) = env::var(ENV_AUTH_MINT_MAX_BAT) {
            if let Ok(max_bat) = max_bat_str.parse() {
                self.mint_max_bat = max_bat;
            }
        }

        if let Ok(enabled_mint_str) = env::var(ENV_AUTH_ENABLED_MINT) {
            if let Ok(enabled) = enabled_mint_str.parse() {
                self.enabled_mint = enabled;
            }
        }

        if let Ok(enabled_melt_str) = env::var(ENV_AUTH_ENABLED_MELT) {
            if let Ok(enabled) = enabled_melt_str.parse() {
                self.enabled_melt = enabled;
            }
        }

        if let Ok(enabled_swap_str) = env::var(ENV_AUTH_ENABLED_SWAP) {
            if let Ok(enabled) = enabled_swap_str.parse() {
                self.enabled_swap = enabled;
            }
        }

        if let Ok(enabled_check_mint_str) = env::var(ENV_AUTH_ENABLED_CHECK_MINT_QUOTE) {
            if let Ok(enabled) = enabled_check_mint_str.parse() {
                self.enabled_check_mint_quote = enabled;
            }
        }

        if let Ok(enabled_check_melt_str) = env::var(ENV_AUTH_ENABLED_CHECK_MELT_QUOTE) {
            if let Ok(enabled) = enabled_check_melt_str.parse() {
                self.enabled_check_melt_quote = enabled;
            }
        }

        if let Ok(enabled_restore_str) = env::var(ENV_AUTH_ENABLED_RESTORE) {
            if let Ok(enabled) = enabled_restore_str.parse() {
                self.enabled_restore = enabled;
            }
        }

        if let Ok(enabled_check_proof_str) = env::var(ENV_AUTH_ENABLED_CHECK_PROOF_STATE) {
            if let Ok(enabled) = enabled_check_proof_str.parse() {
                self.enabled_check_proof_state = enabled;
            }
        }

        self
    }
}
