//! Spark environment variables

use std::env;

use crate::config::Spark;

// Spark environment variables
pub const ENV_SPARK_NETWORK: &str = "CDK_MINTD_SPARK_NETWORK";
pub const ENV_SPARK_MNEMONIC: &str = "CDK_MINTD_SPARK_MNEMONIC";
pub const ENV_SPARK_PASSPHRASE: &str = "CDK_MINTD_SPARK_PASSPHRASE";
pub const ENV_SPARK_API_KEY: &str = "CDK_MINTD_SPARK_API_KEY";
pub const ENV_SPARK_FEE_PERCENT: &str = "CDK_MINTD_SPARK_FEE_PERCENT";
pub const ENV_SPARK_RESERVE_FEE_MIN: &str = "CDK_MINTD_SPARK_RESERVE_FEE_MIN";
pub const ENV_SPARK_RECONNECT_INTERVAL: &str = "CDK_MINTD_SPARK_RECONNECT_INTERVAL";
pub const ENV_SPARK_SPLIT_SECRET_THRESHOLD: &str = "CDK_MINTD_SPARK_SPLIT_SECRET_THRESHOLD";

impl Spark {
    pub fn from_env(mut self) -> Self {
        if let Ok(network) = env::var(ENV_SPARK_NETWORK) {
            self.network = network;
        }

        if let Ok(mnemonic) = env::var(ENV_SPARK_MNEMONIC) {
            self.mnemonic = mnemonic;
        }

        if let Ok(passphrase) = env::var(ENV_SPARK_PASSPHRASE) {
            self.passphrase = Some(passphrase);
        }

        if let Ok(api_key) = env::var(ENV_SPARK_API_KEY) {
            self.api_key = Some(api_key);
        }

        if let Ok(fee_str) = env::var(ENV_SPARK_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_SPARK_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        if let Ok(reconnect_str) = env::var(ENV_SPARK_RECONNECT_INTERVAL) {
            if let Ok(reconnect) = reconnect_str.parse() {
                self.reconnect_interval_seconds = reconnect;
            }
        }

        if let Ok(threshold_str) = env::var(ENV_SPARK_SPLIT_SECRET_THRESHOLD) {
            if let Ok(threshold) = threshold_str.parse() {
                self.split_secret_threshold = threshold;
            }
        }

        self
    }
}
