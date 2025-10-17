//! Configuration for CDK Spark integration

use cdk_common::common::FeeReserve;
use serde::{Deserialize, Serialize};
use spark_wallet::{Network, OperatorPoolConfig, ServiceProviderConfig};

/// Configuration for the Spark Lightning backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparkConfig {
    /// Bitcoin network (mainnet, testnet, regtest, signet)
    pub network: Network,

    /// BIP39 mnemonic phrase for wallet seed
    pub mnemonic: String,

    /// Optional passphrase for the mnemonic
    #[serde(default)]
    pub passphrase: Option<String>,

    /// Directory path for Spark wallet storage
    pub storage_dir: String,

    /// Optional API key for Spark service provider
    #[serde(default)]
    pub api_key: Option<String>,

    /// Operator pool configuration (URLs, etc.)
    #[serde(default)]
    pub operator_pool: Option<OperatorPoolConfig>,

    /// Service provider configuration
    #[serde(default)]
    pub service_provider: Option<ServiceProviderConfig>,

    /// Fee reserve settings
    pub fee_reserve: FeeReserve,

    /// Reconnect interval in seconds for background tasks
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval_seconds: u64,

    /// Split secret threshold for multi-sig operations
    #[serde(default = "default_split_secret_threshold")]
    pub split_secret_threshold: usize,
}

fn default_reconnect_interval() -> u64 {
    30 // 30 seconds
}

fn default_split_secret_threshold() -> usize {
    2 // Default threshold for secret sharing
}

impl SparkConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        // Validate mnemonic
        if self.mnemonic.trim().is_empty() {
            return Err(crate::error::Error::Configuration(
                "Mnemonic cannot be empty".to_string(),
            ));
        }

        // Validate storage directory
        if self.storage_dir.trim().is_empty() {
            return Err(crate::error::Error::Configuration(
                "Storage directory cannot be empty".to_string(),
            ));
        }

        // Validate fee reserve
        if self.fee_reserve.percent_fee_reserve < 0.0 {
            return Err(crate::error::Error::Configuration(
                "Fee reserve percentage cannot be negative".to_string(),
            ));
        }

        Ok(())
    }

    /// Create a default configuration for the given network
    pub fn default_for_network(network: Network, mnemonic: String, storage_dir: String) -> Self {
        Self {
            network,
            mnemonic,
            passphrase: None,
            storage_dir,
            api_key: None,
            operator_pool: None,
            service_provider: None,
            fee_reserve: FeeReserve {
                min_fee_reserve: 10.into(),
                percent_fee_reserve: 0.01,
            },
            reconnect_interval_seconds: default_reconnect_interval(),
            split_secret_threshold: default_split_secret_threshold(),
        }
    }
}

