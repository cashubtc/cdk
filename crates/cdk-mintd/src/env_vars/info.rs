//! Info environment variables

use std::env;

use super::common::*;
use crate::config::Info;

impl Info {
    pub fn from_env(mut self) -> Self {
        // Required fields
        if let Ok(url) = env::var(ENV_URL) {
            self.url = url;
        }

        if let Ok(host) = env::var(ENV_LISTEN_HOST) {
            self.listen_host = host;
        }

        if let Ok(port_str) = env::var(ENV_LISTEN_PORT) {
            if let Ok(port) = port_str.parse() {
                self.listen_port = port;
            }
        }

        if let Ok(mnemonic) = env::var(ENV_MNEMONIC) {
            self.mnemonic = mnemonic;
        }

        if let Ok(cache_seconds_str) = env::var(ENV_CACHE_SECONDS) {
            if let Ok(seconds) = cache_seconds_str.parse() {
                self.http_cache.ttl = Some(seconds);
            }
        }

        if let Ok(extend_cache_str) = env::var(ENV_EXTEND_CACHE_SECONDS) {
            if let Ok(seconds) = extend_cache_str.parse() {
                self.http_cache.tti = Some(seconds);
            }
        }

        if let Ok(fee_str) = env::var(ENV_INPUT_FEE_PPK) {
            if let Ok(fee) = fee_str.parse() {
                self.input_fee_ppk = Some(fee);
            }
        }

        if let Ok(swagger_str) = env::var(ENV_ENABLE_SWAGGER) {
            if let Ok(enable) = swagger_str.parse() {
                self.enable_swagger_ui = Some(enable);
            }
        }

        self.http_cache = self.http_cache.from_env();

        self
    }
}
