//! Environment variables for OHTTP Gateway configuration

use std::env;

use crate::config::OhttpGateway;

// Environment variable names
pub const OHTTP_GATEWAY_ENABLED_ENV_VAR: &str = "CDK_MINTD_OHTTP_GATEWAY_ENABLED";

impl OhttpGateway {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled_str) = env::var(OHTTP_GATEWAY_ENABLED_ENV_VAR) {
            self.enabled = enabled_str.to_lowercase() == "true" || enabled_str == "1";
        }

        self
    }
}
