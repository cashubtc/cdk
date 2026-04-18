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

        self
    }
}
