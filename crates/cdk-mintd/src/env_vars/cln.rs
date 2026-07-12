//! CLN environment variables

use std::env;
use std::path::PathBuf;

use crate::config::Cln;

// CLN environment variables
pub const ENV_CLN_RPC_PATH: &str = "CDK_MINTD_CLN_RPC_PATH";
pub const ENV_CLN_BOLT12: &str = "CDK_MINTD_CLN_BOLT12";
pub const ENV_CLN_FEE_PERCENT: &str = "CDK_MINTD_CLN_FEE_PERCENT";
pub const ENV_CLN_RESERVE_FEE_MIN: &str = "CDK_MINTD_CLN_RESERVE_FEE_MIN";
pub const ENV_CLN_EXPOSE_PRIVATE_CHANNELS: &str = "CDK_MINTD_CLN_EXPOSE_PRIVATE_CHANNELS";

impl Cln {
    pub fn from_env(mut self) -> Self {
        // RPC Path
        if let Ok(path) = env::var(ENV_CLN_RPC_PATH) {
            self.rpc_path = PathBuf::from(path);
        }

        // BOLT12 flag
        if let Ok(bolt12_str) = env::var(ENV_CLN_BOLT12) {
            if let Ok(bolt12) = bolt12_str.parse() {
                self.bolt12 = bolt12;
            }
        }

        // Expose private channels
        if let Ok(expose_str) = env::var(ENV_CLN_EXPOSE_PRIVATE_CHANNELS) {
            if let Ok(expose) = expose_str.parse() {
                self.expose_private_channels = expose;
            }
        }

        // Fee percent
        if let Ok(fee_str) = env::var(ENV_CLN_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        // Reserve fee minimum
        if let Ok(reserve_fee_str) = env::var(ENV_CLN_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use cdk::cdk_payment::MintPayment;
    use cdk::nuts::CurrencyUnit;
    use cdk_sqlite::mint::memory;

    use super::*;
    use crate::config::Settings;
    use crate::setup::PaymentBackendSetup;

    #[tokio::test]
    async fn bolt12_false_removes_bolt12_from_cln_capabilities() {
        let config = {
            let _guard = crate::test_utils::env_lock();
            let previous_bolt12 = std::env::var_os(ENV_CLN_BOLT12);
            std::env::set_var(ENV_CLN_BOLT12, "false");

            let config = Cln {
                rpc_path: PathBuf::from("/nonexistent/lightning-rpc"),
                ..Default::default()
            }
            .from_env();

            match previous_bolt12 {
                Some(value) => std::env::set_var(ENV_CLN_BOLT12, value),
                None => std::env::remove_var(ENV_CLN_BOLT12),
            }

            config
        };
        assert!(!config.bolt12, "the environment override must be applied");

        let kv_store = Arc::new(memory::empty().await.expect("in-memory database"));
        let backend = config
            .setup(
                &Settings::default(),
                CurrencyUnit::Sat,
                None,
                Path::new("."),
                Some(kv_store),
            )
            .await
            .expect("CLN setup does not connect eagerly");

        let capabilities = backend.get_settings().await.expect("CLN capabilities");
        assert!(
            capabilities.bolt12.is_none(),
            "CDK_MINTD_CLN_BOLT12=false must disable BOLT12 capabilities"
        );
    }
}
