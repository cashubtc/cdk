//! CLN environment variables

use std::env;
use std::path::PathBuf;

use crate::config::Cln;

// CLN environment variables
pub const ENV_CLN_RPC_PATH: &str = "CDK_MINTD_PAYMENT_BACKEND_CLN_RPC_PATH";
pub const ENV_CLN_BOLT12: &str = "CDK_MINTD_PAYMENT_BACKEND_CLN_BOLT12";
pub const ENV_CLN_FEE_PERCENT: &str = "CDK_MINTD_PAYMENT_BACKEND_CLN_FEE_PERCENT";
pub const ENV_CLN_RESERVE_FEE_MIN: &str = "CDK_MINTD_PAYMENT_BACKEND_CLN_RESERVE_FEE_MIN";

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
