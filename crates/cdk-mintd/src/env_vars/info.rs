//! Info environment variables

use std::env;
use std::str::FromStr;

use anyhow::{Context, Result};
use cdk_common::common::QuoteTTL;

use super::common::*;
use crate::config::{Info, LoggingOutput};

impl Info {
    pub fn from_env(mut self) -> Result<Self> {
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

        if let Ok(seed) = env::var(ENV_SEED) {
            self.seed = Some(seed);
        }

        if let Ok(mnemonic) = env::var(ENV_MNEMONIC) {
            self.mnemonic = Some(mnemonic);
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

        if let Ok(info_page_str) = env::var(ENV_ENABLE_INFO_PAGE) {
            if let Ok(enable) = info_page_str.parse() {
                self.enable_info_page = Some(enable);
            }
        }

        if let Ok(use_keyset_v2_str) = env::var(ENV_USE_KEYSET_V2) {
            if let Ok(use_keyset_v2) = use_keyset_v2_str.parse() {
                self.use_keyset_v2 = Some(use_keyset_v2);
            }
        }

        if let Ok(interval_str) = env::var(ENV_KEYSET_ROTATION_INTERVAL_SECONDS) {
            self.keyset_rotation_interval_seconds = interval_str.parse().with_context(|| {
                format!(
                    "{ENV_KEYSET_ROTATION_INTERVAL_SECONDS} must be a whole number of seconds; \
                     0 disables keyset auto-rotation"
                )
            })?;
        }

        // Logging configuration
        if let Ok(output_str) = env::var(ENV_LOGGING_OUTPUT) {
            if let Ok(output) = LoggingOutput::from_str(&output_str) {
                self.logging.output = output;
            } else {
                tracing::warn!(
                    "Invalid logging output '{}' in environment variable. Valid options: stdout, file, both",
                    output_str
                );
            }
        }

        if let Ok(console_level) = env::var(ENV_LOGGING_CONSOLE_LEVEL) {
            self.logging.console_level = Some(console_level);
        }

        if let Ok(file_level) = env::var(ENV_LOGGING_FILE_LEVEL) {
            self.logging.file_level = Some(file_level);
        }

        self.http_cache = self.http_cache.from_env();

        // Quote TTL from env
        let mut mint_ttl_env: Option<u64> = None;
        let mut melt_ttl_env: Option<u64> = None;
        if let Ok(mint_ttl_str) = env::var(ENV_QUOTE_TTL_MINT) {
            if let Ok(v) = mint_ttl_str.parse::<u64>() {
                mint_ttl_env = Some(v);
            }
        }
        if let Ok(melt_ttl_str) = env::var(ENV_QUOTE_TTL_MELT) {
            if let Ok(v) = melt_ttl_str.parse::<u64>() {
                melt_ttl_env = Some(v);
            }
        }
        if mint_ttl_env.is_some() || melt_ttl_env.is_some() {
            let current = self.quote_ttl.unwrap_or_default();
            self.quote_ttl = Some(QuoteTTL {
                mint_ttl: mint_ttl_env.unwrap_or(current.mint_ttl),
                melt_ttl: melt_ttl_env.unwrap_or(current.melt_ttl),
            });
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_env_vars() {
        env::remove_var(ENV_KEYSET_ROTATION_INTERVAL_SECONDS);
    }

    #[test]
    fn info_from_env_reads_the_keyset_rotation_interval() {
        let _guard = crate::test_utils::env_lock();
        clear_env_vars();

        env::set_var(ENV_KEYSET_ROTATION_INTERVAL_SECONDS, "3600");

        let info = Info::default().from_env().expect("valid env");
        assert_eq!(info.keyset_rotation_interval_seconds, 3600);

        clear_env_vars();
    }

    /// An operator writing `off` means "disable rotation". Falling back to the
    /// 90-day default would silently rotate keys instead, so the parse fails.
    #[test]
    fn info_from_env_rejects_an_unparsable_keyset_rotation_interval() {
        let _guard = crate::test_utils::env_lock();
        clear_env_vars();

        env::set_var(ENV_KEYSET_ROTATION_INTERVAL_SECONDS, "off");

        let err = Info::default()
            .from_env()
            .expect_err("an unparsable rotation interval must fail configuration");
        assert!(
            err.to_string()
                .contains(ENV_KEYSET_ROTATION_INTERVAL_SECONDS),
            "the error must name the offending variable, got: {err}"
        );

        clear_env_vars();
    }
}
