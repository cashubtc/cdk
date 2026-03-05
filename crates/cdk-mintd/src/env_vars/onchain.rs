//! Onchain environment variables

use std::env;

use crate::config::Onchain;

// Onchain Environment Variables
pub const ONCHAIN_ENABLED_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_ENABLED";
pub const ONCHAIN_MNEMONIC_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_MNEMONIC";
pub const ONCHAIN_NETWORK_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_NETWORK";
pub const ONCHAIN_RPC_URL_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_RPC_URL";
pub const ONCHAIN_RPC_USER_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_RPC_USER";
pub const ONCHAIN_RPC_PASS_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_RPC_PASS";
pub const ONCHAIN_CHAIN_SOURCE_TYPE_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_CHAIN_SOURCE_TYPE";
pub const ONCHAIN_ESPLORA_URL_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_ESPLORA_URL";
pub const ONCHAIN_NUM_CONFS_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_NUM_CONFS";
pub const ONCHAIN_FEE_PERCENT_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_FEE_PERCENT";
pub const ONCHAIN_RESERVE_FEE_MIN_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_RESERVE_FEE_MIN";
pub const ONCHAIN_MIN_RECEIVE_AMOUNT_SAT_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_MIN_RECEIVE_AMOUNT_SAT";

pub const ONCHAIN_POLL_INTERVAL_SECS_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_POLL_INTERVAL_SECS";
pub const ONCHAIN_MAX_BATCH_SIZE_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_MAX_BATCH_SIZE";
pub const ONCHAIN_STANDARD_DEADLINE_SECS_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_STANDARD_DEADLINE_SECS";
pub const ONCHAIN_ECONOMY_DEADLINE_SECS_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_ECONOMY_DEADLINE_SECS";
pub const ONCHAIN_MIN_BATCH_THRESHOLD_ENV_VAR: &str = "CDK_MINTD_ONCHAIN_MIN_BATCH_THRESHOLD";

impl Onchain {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled) = env::var(ONCHAIN_ENABLED_ENV_VAR) {
            if let Ok(enabled) = enabled.parse::<bool>() {
                self.enabled = enabled;
            }
        }

        if let Ok(mnemonic) = env::var(ONCHAIN_MNEMONIC_ENV_VAR) {
            self.mnemonic = Some(mnemonic);
        }

        if let Ok(network) = env::var(ONCHAIN_NETWORK_ENV_VAR) {
            self.network = network;
        }

        if let Ok(rpc_url) = env::var(ONCHAIN_RPC_URL_ENV_VAR) {
            self.rpc_url = rpc_url;
        }

        if let Ok(rpc_user) = env::var(ONCHAIN_RPC_USER_ENV_VAR) {
            self.rpc_user = rpc_user;
        }

        if let Ok(rpc_pass) = env::var(ONCHAIN_RPC_PASS_ENV_VAR) {
            self.rpc_pass = rpc_pass;
        }

        if let Ok(chain_source_type) = env::var(ONCHAIN_CHAIN_SOURCE_TYPE_ENV_VAR) {
            self.chain_source_type = Some(chain_source_type);
        }

        if let Ok(esplora_url) = env::var(ONCHAIN_ESPLORA_URL_ENV_VAR) {
            self.esplora_url = Some(esplora_url);
        }

        if let Ok(num_confs) = env::var(ONCHAIN_NUM_CONFS_ENV_VAR) {
            if let Ok(num_confs) = num_confs.parse::<u32>() {
                self.num_confs = num_confs;
            }
        }

        if let Ok(fee_percent) = env::var(ONCHAIN_FEE_PERCENT_ENV_VAR) {
            if let Ok(fee_percent) = fee_percent.parse::<f32>() {
                self.fee_percent = fee_percent;
            }
        }

        if let Ok(reserve_fee_min) = env::var(ONCHAIN_RESERVE_FEE_MIN_ENV_VAR) {
            if let Ok(reserve_fee_min) = reserve_fee_min.parse::<u64>() {
                self.reserve_fee_min = reserve_fee_min.into();
            }
        }

        if let Ok(min_receive_amount_sat) = env::var(ONCHAIN_MIN_RECEIVE_AMOUNT_SAT_ENV_VAR) {
            if let Ok(min_receive_amount_sat) = min_receive_amount_sat.parse::<u64>() {
                self.min_receive_amount_sat = min_receive_amount_sat;
            }
        }

        if let Ok(poll_interval) = env::var(ONCHAIN_POLL_INTERVAL_SECS_ENV_VAR) {
            if let Ok(poll_interval) = poll_interval.parse::<u64>() {
                self.batch_config.poll_interval_secs = poll_interval;
            }
        }

        if let Ok(max_batch_size) = env::var(ONCHAIN_MAX_BATCH_SIZE_ENV_VAR) {
            if let Ok(max_batch_size) = max_batch_size.parse::<usize>() {
                self.batch_config.max_batch_size = max_batch_size;
            }
        }

        if let Ok(standard_deadline) = env::var(ONCHAIN_STANDARD_DEADLINE_SECS_ENV_VAR) {
            if let Ok(standard_deadline) = standard_deadline.parse::<u64>() {
                self.batch_config.standard_deadline_secs = standard_deadline;
            }
        }

        if let Ok(economy_deadline) = env::var(ONCHAIN_ECONOMY_DEADLINE_SECS_ENV_VAR) {
            if let Ok(economy_deadline) = economy_deadline.parse::<u64>() {
                self.batch_config.economy_deadline_secs = economy_deadline;
            }
        }

        if let Ok(min_batch_threshold) = env::var(ONCHAIN_MIN_BATCH_THRESHOLD_ENV_VAR) {
            if let Ok(min_batch_threshold) = min_batch_threshold.parse::<usize>() {
                self.batch_config.min_batch_threshold = min_batch_threshold;
            }
        }

        self
    }
}
