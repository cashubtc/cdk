//! BDK environment variables

use std::env;

use crate::config::Bdk;

pub const BDK_MNEMONIC_ENV_VAR: &str = "CDK_MINTD_BDK_MNEMONIC";
pub const BDK_NETWORK_ENV_VAR: &str = "CDK_MINTD_BDK_NETWORK";
pub const BDK_BITCOIND_RPC_HOST_ENV_VAR: &str = "CDK_MINTD_BDK_BITCOIND_RPC_HOST";
pub const BDK_BITCOIND_RPC_PORT_ENV_VAR: &str = "CDK_MINTD_BDK_BITCOIND_RPC_PORT";
pub const BDK_BITCOIND_RPC_USER_ENV_VAR: &str = "CDK_MINTD_BDK_BITCOIND_RPC_USER";
pub const BDK_BITCOIND_RPC_PASSWORD_ENV_VAR: &str = "CDK_MINTD_BDK_BITCOIND_RPC_PASSWORD";
pub const BDK_CHAIN_SOURCE_TYPE_ENV_VAR: &str = "CDK_MINTD_BDK_CHAIN_SOURCE_TYPE";
pub const BDK_ESPLORA_URL_ENV_VAR: &str = "CDK_MINTD_BDK_ESPLORA_URL";
pub const BDK_NUM_CONFS_ENV_VAR: &str = "CDK_MINTD_BDK_NUM_CONFS";
pub const BDK_FEE_PERCENT_ENV_VAR: &str = "CDK_MINTD_BDK_FEE_PERCENT";
pub const BDK_RESERVE_FEE_MIN_ENV_VAR: &str = "CDK_MINTD_BDK_RESERVE_FEE_MIN";
pub const BDK_MIN_RECEIVE_AMOUNT_SAT_ENV_VAR: &str = "CDK_MINTD_BDK_MIN_RECEIVE_AMOUNT_SAT";
pub const BDK_SYNC_INTERVAL_SECS_ENV_VAR: &str = "CDK_MINTD_BDK_SYNC_INTERVAL_SECS";
pub const BDK_BATCH_POLL_INTERVAL_SECS_ENV_VAR: &str = "CDK_MINTD_BDK_BATCH_POLL_INTERVAL_SECS";
pub const BDK_BATCH_MAX_BATCH_SIZE_ENV_VAR: &str = "CDK_MINTD_BDK_BATCH_MAX_BATCH_SIZE";
pub const BDK_BATCH_STANDARD_DEADLINE_SECS_ENV_VAR: &str =
    "CDK_MINTD_BDK_BATCH_STANDARD_DEADLINE_SECS";
pub const BDK_BATCH_ECONOMY_DEADLINE_SECS_ENV_VAR: &str =
    "CDK_MINTD_BDK_BATCH_ECONOMY_DEADLINE_SECS";
pub const BDK_BATCH_MIN_BATCH_THRESHOLD_ENV_VAR: &str = "CDK_MINTD_BDK_BATCH_MIN_BATCH_THRESHOLD";

impl Bdk {
    pub fn from_env(mut self) -> Self {
        if let Ok(mnemonic) = env::var(BDK_MNEMONIC_ENV_VAR) {
            self.mnemonic = Some(mnemonic);
        }

        if let Ok(network) = env::var(BDK_NETWORK_ENV_VAR) {
            self.network = Some(network);
        }

        if let Ok(bitcoind_rpc_host) = env::var(BDK_BITCOIND_RPC_HOST_ENV_VAR) {
            self.bitcoind_rpc_host = Some(bitcoind_rpc_host);
        }

        if let Ok(bitcoind_rpc_port) = env::var(BDK_BITCOIND_RPC_PORT_ENV_VAR) {
            if let Ok(bitcoind_rpc_port) = bitcoind_rpc_port.parse::<u16>() {
                self.bitcoind_rpc_port = Some(bitcoind_rpc_port);
            }
        }

        if let Ok(bitcoind_rpc_user) = env::var(BDK_BITCOIND_RPC_USER_ENV_VAR) {
            self.bitcoind_rpc_user = Some(bitcoind_rpc_user);
        }

        if let Ok(bitcoind_rpc_password) = env::var(BDK_BITCOIND_RPC_PASSWORD_ENV_VAR) {
            self.bitcoind_rpc_password = Some(bitcoind_rpc_password);
        }

        if let Ok(chain_source_type) = env::var(BDK_CHAIN_SOURCE_TYPE_ENV_VAR) {
            self.chain_source_type = Some(chain_source_type);
        }

        if let Ok(esplora_url) = env::var(BDK_ESPLORA_URL_ENV_VAR) {
            self.esplora_url = Some(esplora_url);
        }

        if let Ok(num_confs) = env::var(BDK_NUM_CONFS_ENV_VAR) {
            if let Ok(num_confs) = num_confs.parse::<u32>() {
                self.num_confs = num_confs;
            }
        }

        if let Ok(fee_percent) = env::var(BDK_FEE_PERCENT_ENV_VAR) {
            if let Ok(fee_percent) = fee_percent.parse::<f32>() {
                self.fee_percent = fee_percent;
            }
        }

        if let Ok(reserve_fee_min) = env::var(BDK_RESERVE_FEE_MIN_ENV_VAR) {
            if let Ok(reserve_fee_min) = reserve_fee_min.parse::<u64>() {
                self.reserve_fee_min = reserve_fee_min.into();
            }
        }

        if let Ok(min_receive_amount_sat) = env::var(BDK_MIN_RECEIVE_AMOUNT_SAT_ENV_VAR) {
            if let Ok(min_receive_amount_sat) = min_receive_amount_sat.parse::<u64>() {
                self.min_receive_amount_sat = min_receive_amount_sat;
            }
        }

        if let Ok(sync_interval_secs) = env::var(BDK_SYNC_INTERVAL_SECS_ENV_VAR) {
            if let Ok(sync_interval_secs) = sync_interval_secs.parse::<u64>() {
                self.sync_interval_secs = sync_interval_secs;
            }
        }

        if let Ok(poll_interval_secs) = env::var(BDK_BATCH_POLL_INTERVAL_SECS_ENV_VAR) {
            if let Ok(poll_interval_secs) = poll_interval_secs.parse::<u64>() {
                self.batch_config.poll_interval_secs = poll_interval_secs;
            }
        }

        if let Ok(max_batch_size) = env::var(BDK_BATCH_MAX_BATCH_SIZE_ENV_VAR) {
            if let Ok(max_batch_size) = max_batch_size.parse::<usize>() {
                self.batch_config.max_batch_size = max_batch_size;
            }
        }

        if let Ok(standard_deadline_secs) = env::var(BDK_BATCH_STANDARD_DEADLINE_SECS_ENV_VAR) {
            if let Ok(standard_deadline_secs) = standard_deadline_secs.parse::<u64>() {
                self.batch_config.standard_deadline_secs = standard_deadline_secs;
            }
        }

        if let Ok(economy_deadline_secs) = env::var(BDK_BATCH_ECONOMY_DEADLINE_SECS_ENV_VAR) {
            if let Ok(economy_deadline_secs) = economy_deadline_secs.parse::<u64>() {
                self.batch_config.economy_deadline_secs = economy_deadline_secs;
            }
        }

        if let Ok(min_batch_threshold) = env::var(BDK_BATCH_MIN_BATCH_THRESHOLD_ENV_VAR) {
            if let Ok(min_batch_threshold) = min_batch_threshold.parse::<usize>() {
                self.batch_config.min_batch_threshold = min_batch_threshold;
            }
        }

        self
    }
}
