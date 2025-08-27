//! LDK Node environment variables

use std::env;

use crate::config::LdkNode;

// LDK Node Environment Variables
pub const LDK_NODE_FEE_PERCENT_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_FEE_PERCENT";
pub const LDK_NODE_RESERVE_FEE_MIN_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_RESERVE_FEE_MIN";
pub const LDK_NODE_BITCOIN_NETWORK_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_BITCOIN_NETWORK";
pub const LDK_NODE_CHAIN_SOURCE_TYPE_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_CHAIN_SOURCE_TYPE";
pub const LDK_NODE_ESPLORA_URL_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_ESPLORA_URL";
pub const LDK_NODE_BITCOIND_RPC_HOST_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_BITCOIND_RPC_HOST";
pub const LDK_NODE_BITCOIND_RPC_PORT_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_BITCOIND_RPC_PORT";
pub const LDK_NODE_BITCOIND_RPC_USER_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_BITCOIND_RPC_USER";
pub const LDK_NODE_BITCOIND_RPC_PASSWORD_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_BITCOIND_RPC_PASSWORD";
pub const LDK_NODE_STORAGE_DIR_PATH_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_STORAGE_DIR_PATH";
pub const LDK_NODE_LDK_NODE_HOST_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_LDK_NODE_HOST";
pub const LDK_NODE_LDK_NODE_PORT_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_LDK_NODE_PORT";
pub const LDK_NODE_GOSSIP_SOURCE_TYPE_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE";
pub const LDK_NODE_RGS_URL_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_RGS_URL";
pub const LDK_NODE_WEBSERVER_HOST_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_WEBSERVER_HOST";
pub const LDK_NODE_WEBSERVER_PORT_ENV_VAR: &str = "CDK_MINTD_LDK_NODE_WEBSERVER_PORT";

impl LdkNode {
    pub fn from_env(mut self) -> Self {
        if let Ok(fee_percent) = env::var(LDK_NODE_FEE_PERCENT_ENV_VAR) {
            if let Ok(fee_percent) = fee_percent.parse::<f32>() {
                self.fee_percent = fee_percent;
            }
        }

        if let Ok(reserve_fee_min) = env::var(LDK_NODE_RESERVE_FEE_MIN_ENV_VAR) {
            if let Ok(reserve_fee_min) = reserve_fee_min.parse::<u64>() {
                self.reserve_fee_min = reserve_fee_min.into();
            }
        }

        if let Ok(bitcoin_network) = env::var(LDK_NODE_BITCOIN_NETWORK_ENV_VAR) {
            self.bitcoin_network = Some(bitcoin_network);
        }

        if let Ok(chain_source_type) = env::var(LDK_NODE_CHAIN_SOURCE_TYPE_ENV_VAR) {
            self.chain_source_type = Some(chain_source_type);
        }

        if let Ok(esplora_url) = env::var(LDK_NODE_ESPLORA_URL_ENV_VAR) {
            self.esplora_url = Some(esplora_url);
        }

        if let Ok(bitcoind_rpc_host) = env::var(LDK_NODE_BITCOIND_RPC_HOST_ENV_VAR) {
            self.bitcoind_rpc_host = Some(bitcoind_rpc_host);
        }

        if let Ok(bitcoind_rpc_port) = env::var(LDK_NODE_BITCOIND_RPC_PORT_ENV_VAR) {
            if let Ok(bitcoind_rpc_port) = bitcoind_rpc_port.parse::<u16>() {
                self.bitcoind_rpc_port = Some(bitcoind_rpc_port);
            }
        }

        if let Ok(bitcoind_rpc_user) = env::var(LDK_NODE_BITCOIND_RPC_USER_ENV_VAR) {
            self.bitcoind_rpc_user = Some(bitcoind_rpc_user);
        }

        if let Ok(bitcoind_rpc_password) = env::var(LDK_NODE_BITCOIND_RPC_PASSWORD_ENV_VAR) {
            self.bitcoind_rpc_password = Some(bitcoind_rpc_password);
        }

        if let Ok(storage_dir_path) = env::var(LDK_NODE_STORAGE_DIR_PATH_ENV_VAR) {
            self.storage_dir_path = Some(storage_dir_path);
        }

        if let Ok(ldk_node_host) = env::var(LDK_NODE_LDK_NODE_HOST_ENV_VAR) {
            self.ldk_node_host = Some(ldk_node_host);
        }

        if let Ok(ldk_node_port) = env::var(LDK_NODE_LDK_NODE_PORT_ENV_VAR) {
            if let Ok(ldk_node_port) = ldk_node_port.parse::<u16>() {
                self.ldk_node_port = Some(ldk_node_port);
            }
        }

        if let Ok(gossip_source_type) = env::var(LDK_NODE_GOSSIP_SOURCE_TYPE_ENV_VAR) {
            self.gossip_source_type = Some(gossip_source_type);
        }

        if let Ok(rgs_url) = env::var(LDK_NODE_RGS_URL_ENV_VAR) {
            self.rgs_url = Some(rgs_url);
        }

        if let Ok(webserver_host) = env::var(LDK_NODE_WEBSERVER_HOST_ENV_VAR) {
            self.webserver_host = Some(webserver_host);
        }

        if let Ok(webserver_port) = env::var(LDK_NODE_WEBSERVER_PORT_ENV_VAR) {
            if let Ok(webserver_port) = webserver_port.parse::<u16>() {
                self.webserver_port = Some(webserver_port);
            }
        }

        self
    }
}
