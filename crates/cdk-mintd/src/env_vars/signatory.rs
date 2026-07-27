//! Remote signatory environment variables

use std::env;

use anyhow::{Context, Result};

use super::common::*;
use crate::config::Signatory;

impl Signatory {
    pub fn from_env(mut self) -> Result<Self> {
        if let Ok(enabled) = env::var(ENV_SIGNATORY_ENABLED) {
            if let Ok(enabled) = enabled.parse() {
                self.enabled = enabled;
            }
        }

        if let Ok(addr) = env::var(ENV_SIGNATORY_ADDRESS) {
            self.address = addr;
        }

        if let Ok(port) = env::var(ENV_SIGNATORY_PORT) {
            if let Ok(port) = port.parse() {
                self.port = port;
            }
        }

        if let Ok(tls_dir) = env::var(ENV_SIGNATORY_TLS_DIR) {
            self.tls_dir = Some(tls_dir.into());
        }

        if let Ok(allow_insecure) = env::var(ENV_SIGNATORY_ALLOW_INSECURE) {
            if let Ok(allow_insecure) = allow_insecure.parse() {
                self.allow_insecure = allow_insecure;
            }
        }

        // Hard failure rather than a silent fallback: an unparsable value here
        // would otherwise leave the 90-day default in place, so an operator
        // trying to disable auto-rotation would get keys rotating instead.
        if let Ok(interval_str) = env::var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS) {
            let interval = interval_str.parse().with_context(|| {
                format!(
                    "{ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS} must be a whole number of \
                     seconds; 0 disables keyset auto-rotation"
                )
            })?;
            self.keyset_rotation_interval_seconds = Some(interval);
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_utils::env_lock()
    }

    fn clear_env_vars() {
        env::remove_var(ENV_SIGNATORY_ENABLED);
        env::remove_var(ENV_SIGNATORY_ADDRESS);
        env::remove_var(ENV_SIGNATORY_PORT);
        env::remove_var(ENV_SIGNATORY_TLS_DIR);
        env::remove_var(ENV_SIGNATORY_ALLOW_INSECURE);
        env::remove_var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS);
    }

    #[test]
    fn signatory_from_env_reads_enabled_and_connection_fields() {
        let _guard = env_lock();
        clear_env_vars();

        env::set_var(ENV_SIGNATORY_ENABLED, "true");
        env::set_var(ENV_SIGNATORY_ADDRESS, "0.0.0.0");
        env::set_var(ENV_SIGNATORY_PORT, "15061");
        env::set_var(ENV_SIGNATORY_TLS_DIR, "/var/lib/cdk/signatory-tls");
        env::set_var(ENV_SIGNATORY_ALLOW_INSECURE, "true");
        env::set_var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS, "7776000");

        let signatory = Signatory::default().from_env().expect("valid env");

        assert!(signatory.enabled);
        assert_eq!(signatory.address, "0.0.0.0");
        assert_eq!(signatory.port, 15061);
        assert_eq!(
            signatory.tls_dir,
            Some(PathBuf::from("/var/lib/cdk/signatory-tls"))
        );
        assert!(signatory.allow_insecure);
        assert_eq!(signatory.keyset_rotation_interval_seconds, Some(7776000));

        clear_env_vars();
    }

    /// An operator writing `off` means "disable rotation". Falling back to the
    /// 90-day default would silently rotate keys instead, so the parse fails.
    #[test]
    fn signatory_from_env_rejects_unparsable_rotation_interval() {
        let _guard = env_lock();
        clear_env_vars();

        env::set_var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS, "off");

        let err = Signatory::default()
            .from_env()
            .expect_err("an unparsable rotation interval must fail configuration");
        assert!(
            err.to_string()
                .contains(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS),
            "the error must name the offending variable, got: {err}"
        );

        clear_env_vars();
    }
}
