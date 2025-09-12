//! Environment variables for OHTTP Gateway configuration

use std::env;

use crate::config::OhttpGateway;

// Environment variable names
pub const OHTTP_GATEWAY_ENABLED_ENV_VAR: &str = "CDK_MINTD_OHTTP_GATEWAY_ENABLED";
pub const OHTTP_GATEWAY_URL_ENV_VAR: &str = "CDK_MINTD_OHTTP_GATEWAY_URL";

impl OhttpGateway {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled_str) = env::var(OHTTP_GATEWAY_ENABLED_ENV_VAR) {
            self.enabled = enabled_str.to_lowercase() == "true" || enabled_str == "1";
        }

        if let Ok(gateway_url) = env::var(OHTTP_GATEWAY_URL_ENV_VAR) {
            self.gateway_url = Some(gateway_url);
        }

        self
    }
}
