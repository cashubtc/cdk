//! Auth env

use std::env;

use crate::config::Auth;

pub const ENV_AUTH_ENABLED: &str = "CDK_MINTD_AUTH_ENABLED";
pub const ENV_AUTH_OPENID_DISCOVERY: &str = "CDK_MINTD_AUTH_OPENID_DISCOVERY";
pub const ENV_AUTH_OPENID_CLIENT_ID: &str = "CDK_MINTD_AUTH_OPENID_CLIENT_ID";
pub const ENV_AUTH_MINT_MAX_BAT: &str = "CDK_MINTD_AUTH_MINT_MAX_BAT";
pub const ENV_AUTH_MINT: &str = "CDK_MINTD_AUTH_MINT";
pub const ENV_AUTH_GET_MINT_QUOTE: &str = "CDK_MINTD_AUTH_GET_MINT_QUOTE";
pub const ENV_AUTH_CHECK_MINT_QUOTE: &str = "CDK_MINTD_AUTH_CHECK_MINT_QUOTE";
pub const ENV_AUTH_MELT: &str = "CDK_MINTD_AUTH_MELT";
pub const ENV_AUTH_GET_MELT_QUOTE: &str = "CDK_MINTD_AUTH_GET_MELT_QUOTE";
pub const ENV_AUTH_CHECK_MELT_QUOTE: &str = "CDK_MINTD_AUTH_CHECK_MELT_QUOTE";
pub const ENV_AUTH_SWAP: &str = "CDK_MINTD_AUTH_SWAP";
pub const ENV_AUTH_RESTORE: &str = "CDK_MINTD_AUTH_RESTORE";
pub const ENV_AUTH_CHECK_PROOF_STATE: &str = "CDK_MINTD_AUTH_CHECK_PROOF_STATE";
pub const ENV_AUTH_WEBSOCKET: &str = "CDK_MINTD_AUTH_WEBSOCKET";
pub const ENV_AUTH_WS_MINT_QUOTE: &str = "CDK_MINTD_AUTH_WS_MINT_QUOTE";
pub const ENV_AUTH_WS_MELT_QUOTE: &str = "CDK_MINTD_AUTH_WS_MELT_QUOTE";
pub const ENV_AUTH_WS_PROOF_STATE: &str = "CDK_MINTD_AUTH_WS_PROOF_STATE";

impl Auth {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled_str) = env::var(ENV_AUTH_ENABLED) {
            if let Ok(enabled) = enabled_str.parse() {
                self.auth_enabled = enabled;
            }
        }

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

        if let Ok(mint_str) = env::var(ENV_AUTH_MINT) {
            if let Ok(auth_type) = mint_str.parse() {
                self.mint = auth_type;
            }
        }

        if let Ok(get_mint_quote_str) = env::var(ENV_AUTH_GET_MINT_QUOTE) {
            if let Ok(auth_type) = get_mint_quote_str.parse() {
                self.get_mint_quote = auth_type;
            }
        }

        if let Ok(check_mint_quote_str) = env::var(ENV_AUTH_CHECK_MINT_QUOTE) {
            if let Ok(auth_type) = check_mint_quote_str.parse() {
                self.check_mint_quote = auth_type;
            }
        }

        if let Ok(melt_str) = env::var(ENV_AUTH_MELT) {
            if let Ok(auth_type) = melt_str.parse() {
                self.melt = auth_type;
            }
        }

        if let Ok(get_melt_quote_str) = env::var(ENV_AUTH_GET_MELT_QUOTE) {
            if let Ok(auth_type) = get_melt_quote_str.parse() {
                self.get_melt_quote = auth_type;
            }
        }

        if let Ok(check_melt_quote_str) = env::var(ENV_AUTH_CHECK_MELT_QUOTE) {
            if let Ok(auth_type) = check_melt_quote_str.parse() {
                self.check_melt_quote = auth_type;
            }
        }

        if let Ok(swap_str) = env::var(ENV_AUTH_SWAP) {
            if let Ok(auth_type) = swap_str.parse() {
                self.swap = auth_type;
            }
        }

        if let Ok(restore_str) = env::var(ENV_AUTH_RESTORE) {
            if let Ok(auth_type) = restore_str.parse() {
                self.restore = auth_type;
            }
        }

        if let Ok(check_proof_state_str) = env::var(ENV_AUTH_CHECK_PROOF_STATE) {
            if let Ok(auth_type) = check_proof_state_str.parse() {
                self.check_proof_state = auth_type;
            }
        }

        if let Ok(ws_auth_str) = env::var(ENV_AUTH_WEBSOCKET) {
            if let Ok(auth_type) = ws_auth_str.parse() {
                self.websocket_auth = auth_type;
            }
        }

        self
    }
}
