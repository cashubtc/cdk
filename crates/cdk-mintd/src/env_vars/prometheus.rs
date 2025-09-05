//! Prometheus environment variables

use std::env;

use crate::config::Prometheus;

pub const ENV_PROMETHEUS_ENABLED: &str = "CDK_MINTD_PROMETHEUS_ENABLED";
pub const ENV_PROMETHEUS_ADDRESS: &str = "CDK_MINTD_PROMETHEUS_ADDRESS";
pub const ENV_PROMETHEUS_PORT: &str = "CDK_MINTD_PROMETHEUS_PORT";

impl Prometheus {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled_str) = env::var(ENV_PROMETHEUS_ENABLED) {
            if let Ok(enabled) = enabled_str.parse() {
                self.enabled = enabled;
            }
        }

        if let Ok(address) = env::var(ENV_PROMETHEUS_ADDRESS) {
            self.address = Some(address);
        }

        if let Ok(port_str) = env::var(ENV_PROMETHEUS_PORT) {
            if let Ok(port) = port_str.parse() {
                self.port = Some(port);
            }
        }

        self
    }
}
