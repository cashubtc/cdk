//! Remote signatory environment variables

use std::env;

use super::common::*;
use crate::config::Signatory;

impl Signatory {
    pub fn from_env(mut self) -> Self {
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

        if let Ok(interval_str) = env::var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS) {
            if let Ok(interval) = interval_str.parse() {
                self.keyset_rotation_interval_seconds = Some(interval);
            }
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
        // Share the process-wide env lock so this test does not race other
        // tests that manipulate environment variables.
        let _guard = crate::test_utils::env_lock();
        clear_env_vars();

        env::set_var(ENV_SIGNATORY_ENABLED, "true");
        env::set_var(ENV_SIGNATORY_ADDRESS, "0.0.0.0");
        env::set_var(ENV_SIGNATORY_PORT, "15061");
        env::set_var(ENV_SIGNATORY_TLS_DIR, "/var/lib/cdk/signatory-tls");
        env::set_var(ENV_SIGNATORY_ALLOW_INSECURE, "true");
        env::set_var(ENV_SIGNATORY_KEYSET_ROTATION_INTERVAL_SECONDS, "7776000");

        let signatory = Signatory::default().from_env();

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
}
