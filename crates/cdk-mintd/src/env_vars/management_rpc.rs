//! Management RPC environment variables

use std::env;

use crate::config::MintManagementRpc;

// Mint RPC Server environment variables
pub const ENV_MINT_MANAGEMENT_ENABLED: &str = "CDK_MINTD_MINT_MANAGEMENT_ENABLED";
pub const ENV_MINT_MANAGEMENT_ADDRESS: &str = "CDK_MINTD_MANAGEMENT_ADDRESS";
pub const ENV_MINT_MANAGEMENT_PORT: &str = "CDK_MINTD_MANAGEMENT_PORT";
pub const ENV_MINT_MANAGEMENT_TLS_DIR_PATH: &str = "CDK_MINTD_MANAGEMENT_TLS_DIR_PATH";

impl MintManagementRpc {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled) = env::var(ENV_MINT_MANAGEMENT_ENABLED) {
            if let Ok(enabled) = enabled.parse() {
                self.enabled = enabled;
            }
        }

        if let Ok(address) = env::var(ENV_MINT_MANAGEMENT_ADDRESS) {
            self.address = Some(address);
        }

        if let Ok(port) = env::var(ENV_MINT_MANAGEMENT_PORT) {
            if let Ok(port) = port.parse::<u16>() {
                self.port = Some(port);
            }
        }

        if let Ok(tls_path) = env::var(ENV_MINT_MANAGEMENT_TLS_DIR_PATH) {
            self.tls_dir_path = Some(tls_path.into());
        }

        self
    }
}
