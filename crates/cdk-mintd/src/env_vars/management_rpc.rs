//! Management RPC environment variables

use std::env;

use crate::config::MintManagementRpc;

// Mint RPC Server environment variables
pub const ENV_MINT_MANAGEMENT_ENABLED: &str = "CDK_MINTD_MANAGEMENT_ENABLED";
pub const ENV_MINT_MANAGEMENT_ENABLED_LEGACY: &str = "CDK_MINTD_MINT_MANAGEMENT_ENABLED";
pub const ENV_MINT_MANAGEMENT_ADDRESS: &str = "CDK_MINTD_MANAGEMENT_ADDRESS";
pub const ENV_MINT_MANAGEMENT_PORT: &str = "CDK_MINTD_MANAGEMENT_PORT";
pub const ENV_MINT_MANAGEMENT_TLS_DIR: &str = "CDK_MINTD_MANAGEMENT_TLS_DIR";
pub const ENV_MINT_MANAGEMENT_ALLOW_INSECURE: &str = "CDK_MINTD_MANAGEMENT_ALLOW_INSECURE";
pub const ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE: &str =
    "CDK_MINTD_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE";

impl MintManagementRpc {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled) = env::var(ENV_MINT_MANAGEMENT_ENABLED)
            .or_else(|_| env::var(ENV_MINT_MANAGEMENT_ENABLED_LEGACY))
        {
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

        if let Ok(tls_dir) = env::var(ENV_MINT_MANAGEMENT_TLS_DIR) {
            self.tls_dir = Some(tls_dir.into());
        }

        if let Ok(allow_insecure) = env::var(ENV_MINT_MANAGEMENT_ALLOW_INSECURE) {
            if let Ok(allow_insecure) = allow_insecure.parse() {
                self.allow_insecure = allow_insecure;
            }
        }

        if let Ok(allow_override) = env::var(ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE)
        {
            self.allow_mint_quote_payment_override = allow_override.parse().unwrap_or(false);
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_utils::env_lock()
    }

    fn clear_env_vars() {
        env::remove_var(ENV_MINT_MANAGEMENT_ENABLED);
        env::remove_var(ENV_MINT_MANAGEMENT_ENABLED_LEGACY);
        env::remove_var(ENV_MINT_MANAGEMENT_ADDRESS);
        env::remove_var(ENV_MINT_MANAGEMENT_PORT);
        env::remove_var(ENV_MINT_MANAGEMENT_TLS_DIR);
        env::remove_var(ENV_MINT_MANAGEMENT_ALLOW_INSECURE);
        env::remove_var(ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE);
    }

    #[test]
    fn management_env_var_names_share_consistent_prefix() {
        let names = [
            ENV_MINT_MANAGEMENT_ENABLED,
            ENV_MINT_MANAGEMENT_ADDRESS,
            ENV_MINT_MANAGEMENT_PORT,
            ENV_MINT_MANAGEMENT_TLS_DIR,
            ENV_MINT_MANAGEMENT_ALLOW_INSECURE,
            ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE,
        ];

        let prefixes: BTreeSet<&str> = names
            .iter()
            .map(|name| {
                let idx = name
                    .find("MANAGEMENT")
                    .expect("management env var name should contain MANAGEMENT");
                &name[..idx]
            })
            .collect();

        assert_eq!(
            prefixes.len(),
            1,
            "inconsistent management RPC env var prefixes: {prefixes:?}"
        );
    }

    #[test]
    fn management_rpc_from_env_reads_canonical_env_vars() {
        let _guard = env_lock();
        clear_env_vars();

        env::set_var(ENV_MINT_MANAGEMENT_ENABLED, "true");
        env::set_var(ENV_MINT_MANAGEMENT_ADDRESS, "0.0.0.0");
        env::set_var(ENV_MINT_MANAGEMENT_PORT, "10000");
        env::set_var(ENV_MINT_MANAGEMENT_TLS_DIR, "/var/lib/cdk/tls");
        env::set_var(ENV_MINT_MANAGEMENT_ALLOW_INSECURE, "true");
        env::set_var(
            ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE,
            "true",
        );

        let management_rpc = MintManagementRpc::default().from_env();

        assert!(management_rpc.enabled);
        assert_eq!(management_rpc.address.as_deref(), Some("0.0.0.0"));
        assert_eq!(management_rpc.port, Some(10000));
        assert_eq!(
            management_rpc.tls_dir,
            Some(PathBuf::from("/var/lib/cdk/tls"))
        );
        assert!(management_rpc.allow_insecure);
        assert!(management_rpc.allow_mint_quote_payment_override);

        clear_env_vars();
    }

    #[test]
    fn management_rpc_from_env_still_reads_legacy_enabled_env_var() {
        let _guard = env_lock();
        clear_env_vars();

        env::set_var(ENV_MINT_MANAGEMENT_ENABLED_LEGACY, "true");

        let management_rpc = MintManagementRpc::default().from_env();

        assert!(management_rpc.enabled);

        clear_env_vars();
    }

    #[test]
    fn management_rpc_from_env_allows_no_tls_configuration() {
        let _guard = env_lock();
        clear_env_vars();

        env::set_var(ENV_MINT_MANAGEMENT_ENABLED, "true");

        let management_rpc = MintManagementRpc::default().from_env();

        assert!(management_rpc.enabled);
        assert_eq!(management_rpc.tls_dir, None);
        assert!(!management_rpc.allow_insecure);
        assert!(!management_rpc.allow_mint_quote_payment_override);

        clear_env_vars();
    }

    #[test]
    fn management_rpc_payment_override_env_fails_closed() {
        let _guard = env_lock();
        clear_env_vars();
        let management_rpc = MintManagementRpc {
            allow_mint_quote_payment_override: true,
            ..Default::default()
        };

        env::set_var(
            ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE,
            "false",
        );
        assert!(
            !management_rpc
                .clone()
                .from_env()
                .allow_mint_quote_payment_override
        );

        env::set_var(
            ENV_MINT_MANAGEMENT_ALLOW_MINT_QUOTE_PAYMENT_OVERRIDE,
            "not-a-boolean",
        );
        assert!(!management_rpc.from_env().allow_mint_quote_payment_override);

        clear_env_vars();
    }
}
