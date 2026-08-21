use std::fmt;
use std::path::PathBuf;

use bitcoin::hashes::{sha256, Hash};
use cdk::nuts::{CurrencyUnit, PublicKey};
use cdk::Amount;
use cdk_axum::cache;
use cdk_common::common::QuoteTTL;
use cdk_common::redact::url_for_logs;
use config::{Config, ConfigError, File, FileFormat};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoggingOutput {
    /// Log to stderr only
    Stderr,
    /// Log to file only
    File,
    /// Log to both stderr and file (default)
    #[default]
    Both,
}

impl std::str::FromStr for LoggingOutput {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stderr" => Ok(LoggingOutput::Stderr),
            "file" => Ok(LoggingOutput::File),
            "both" => Ok(LoggingOutput::Both),
            _ => Err(format!(
                "Unknown logging output: {s}. Valid options: stdout, file, both"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoggingConfig {
    /// Where to output logs: stdout, file, or both
    #[serde(default)]
    pub output: LoggingOutput,
    /// Log level for console output (when stdout or both)
    pub console_level: Option<String>,
    /// Log level for file output (when file or both)
    pub file_level: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    pub url: String,
    pub listen_host: String,
    pub listen_port: u16,
    /// Overrides mnemonic
    pub seed: Option<String>,
    pub mnemonic: Option<String>,
    pub input_fee_ppk: Option<u64>,
    /// Use keyset v2
    pub use_keyset_v2: Option<bool>,

    pub http_cache: cache::Config,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// When this is set to true, the mint exposes a very simple info page at `/`
    /// showing the mint name and description.
    ///
    /// This requires `mintd` was built with the `info-page` feature flag.
    pub enable_info_page: Option<bool>,

    /// Optional persisted quote TTL values (seconds) to initialize the database with
    /// when RPC is disabled or on first-run when RPC is enabled.
    /// If not provided, defaults are used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_ttl: Option<QuoteTTL>,
}

impl Default for Info {
    fn default() -> Self {
        Info {
            url: String::new(),
            listen_host: "127.0.0.1".to_string(),
            listen_port: 8091, // Default to port 8091 instead of 0
            seed: None,
            mnemonic: None,
            input_fee_ppk: None,
            use_keyset_v2: None,
            http_cache: cache::Config::default(),
            enable_info_page: Some(true),
            logging: LoggingConfig::default(),
            quote_ttl: None,
        }
    }
}

impl std::fmt::Debug for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Use a fallback approach that won't panic
        let mnemonic_display: String = {
            if let Some(mnemonic) = self.mnemonic.as_ref() {
                let hash = sha256::Hash::hash(mnemonic.as_bytes());
                format!("<hashed: {hash}>")
            } else {
                "<not set>".to_string()
            }
        };

        f.debug_struct("Info")
            .field("url", &self.url)
            .field("listen_host", &self.listen_host)
            .field("listen_port", &self.listen_port)
            .field("mnemonic", &mnemonic_display)
            .field("input_fee_ppk", &self.input_fee_ppk)
            .field("use_keyset_v2", &self.use_keyset_v2)
            .field("http_cache", &self.http_cache)
            .field("logging", &self.logging)
            .field("enable_info_page", &self.enable_info_page)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Signatory {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_signatory_address")]
    pub address: String,
    #[serde(default = "default_signatory_port")]
    pub port: u16,
    #[serde(default)]
    pub tls_dir: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure: bool,
}

impl Default for Signatory {
    fn default() -> Self {
        Self {
            enabled: false,
            address: default_signatory_address(),
            port: default_signatory_port(),
            tls_dir: None,
            allow_insecure: false,
        }
    }
}

fn default_signatory_address() -> String {
    "127.0.0.1".to_string()
}

fn default_signatory_port() -> u16 {
    15060
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaymentBackendType {
    #[default]
    None,
    #[cfg(feature = "cln")]
    Cln,
    #[cfg(feature = "fakewallet")]
    FakeWallet,
    #[cfg(feature = "lnd")]
    Lnd,
    #[cfg(feature = "ldk-node")]
    #[serde(alias = "ldk-node")]
    LdkNode,
    #[cfg(feature = "grpc-processor")]
    GrpcProcessor,
}

impl std::str::FromStr for PaymentBackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            #[cfg(feature = "cln")]
            "cln" => Ok(PaymentBackendType::Cln),
            #[cfg(feature = "fakewallet")]
            "fakewallet" => Ok(PaymentBackendType::FakeWallet),
            #[cfg(feature = "lnd")]
            "lnd" => Ok(PaymentBackendType::Lnd),
            #[cfg(feature = "ldk-node")]
            "ldk-node" | "ldknode" => Ok(PaymentBackendType::LdkNode),
            #[cfg(feature = "grpc-processor")]
            "grpcprocessor" => Ok(PaymentBackendType::GrpcProcessor),
            _ => Err(format!("Unknown payment backend: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PaymentBackend {
    pub backend: PaymentBackendType,
    #[serde(default)]
    pub unit: CurrencyUnit,
    pub invoice_description: Option<String>,
    pub min_mint: Amount,
    pub max_mint: Amount,
    pub min_melt: Amount,
    pub max_melt: Amount,
}

impl Default for PaymentBackend {
    fn default() -> Self {
        PaymentBackend {
            backend: PaymentBackendType::default(),
            unit: CurrencyUnit::default(),
            invoice_description: None,
            min_mint: 1.into(),
            max_mint: 500_000.into(),
            min_melt: 1.into(),
            max_melt: 500_000.into(),
        }
    }
}

fn deserialize_payment_backend<'de, D>(deserializer: D) -> Result<Vec<PaymentBackend>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum PaymentBackendOneOrMany {
        Many(Vec<PaymentBackend>),
        One(PaymentBackend),
    }

    match PaymentBackendOneOrMany::deserialize(deserializer)? {
        PaymentBackendOneOrMany::Many(backends) => Ok(backends),
        PaymentBackendOneOrMany::One(backend) => Ok(vec![backend]),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnchainBackend {
    #[default]
    None,
    #[cfg(feature = "bdk")]
    Bdk,
    #[cfg(feature = "fakewallet")]
    FakeWallet,
}

impl std::str::FromStr for OnchainBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(OnchainBackend::None),
            #[cfg(feature = "bdk")]
            "bdk" => Ok(OnchainBackend::Bdk),
            #[cfg(feature = "fakewallet")]
            "fakewallet" => Ok(OnchainBackend::FakeWallet),
            _ => Err(format!("Unknown Onchain backend: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Onchain {
    pub onchain_backend: OnchainBackend,
    pub min_mint: Amount,
    pub max_mint: Amount,
    pub min_melt: Amount,
    pub max_melt: Amount,
}

impl Default for Onchain {
    fn default() -> Self {
        Onchain {
            onchain_backend: OnchainBackend::default(),
            min_mint: 1.into(),
            max_mint: 500_000.into(),
            min_melt: 1.into(),
            max_melt: 500_000.into(),
        }
    }
}

#[cfg(feature = "bdk")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// How often the batch processor wakes up to check for ready intents
    #[serde(default = "default_bdk_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// Maximum number of intents to include in a single batch
    #[serde(default = "default_bdk_max_batch_size")]
    pub max_batch_size: usize,
    /// Average block interval used to derive default delayed tier deadlines.
    #[serde(default = "default_bdk_target_block_time_secs")]
    pub target_block_time_secs: u64,
    /// Optional override for how long standard-tier intents wait before being eligible
    #[serde(default)]
    pub standard_deadline_secs: Option<u64>,
    /// Optional override for how long economy-tier intents wait before being eligible
    #[serde(default)]
    pub economy_deadline_secs: Option<u64>,
    /// Fee tiers exposed in melt quotes. Order determines fee_index values.
    #[serde(default = "default_bdk_fee_options")]
    pub fee_options: Vec<String>,
    /// Quote-time fallback fee rate used when chain estimation fails, in sat/vB.
    #[serde(default = "default_bdk_fee_fallback_sat_per_vb")]
    pub fee_fallback_sat_per_vb: f64,
    /// Fee-rate cache TTL, in seconds.
    #[serde(default = "default_bdk_fee_cache_ttl_secs")]
    pub fee_cache_ttl_secs: u64,
    /// Maximum input count reserved for a quote estimate.
    #[serde(default = "default_bdk_quote_max_input_count")]
    pub quote_max_input_count: usize,
    /// Fixed safety margin added to quote-time fee estimates, in sats.
    #[serde(default = "default_bdk_quote_fixed_safety_sat")]
    pub quote_fixed_safety_sat: u64,
    /// Multiplicative safety margin applied after the raw quote fee estimate.
    #[serde(default = "default_bdk_quote_safety_multiplier")]
    pub quote_safety_multiplier: f64,
}

#[cfg(feature = "bdk")]
impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_bdk_poll_interval_secs(),
            max_batch_size: default_bdk_max_batch_size(),
            target_block_time_secs: default_bdk_target_block_time_secs(),
            standard_deadline_secs: None,
            economy_deadline_secs: None,
            fee_options: default_bdk_fee_options(),
            fee_fallback_sat_per_vb: default_bdk_fee_fallback_sat_per_vb(),
            fee_cache_ttl_secs: default_bdk_fee_cache_ttl_secs(),
            quote_max_input_count: default_bdk_quote_max_input_count(),
            quote_fixed_safety_sat: default_bdk_quote_fixed_safety_sat(),
            quote_safety_multiplier: default_bdk_quote_safety_multiplier(),
        }
    }
}

#[cfg(feature = "bdk")]
#[derive(Clone, Serialize, Deserialize)]
pub struct Bdk {
    /// Fee percentage (e.g., 0.02 for 2%)
    #[serde(default = "default_fee_percent")]
    pub fee_percent: f32,
    /// Minimum reserve fee
    #[serde(default = "default_reserve_fee_min")]
    pub reserve_fee_min: Amount,
    /// Bitcoin network (mainnet, testnet, signet, regtest)
    pub network: Option<String>,
    /// Chain source type ("esplora", "electrum", or "bitcoinrpc"; defaults to "bitcoinrpc")
    pub chain_source_type: Option<String>,
    /// Esplora URL (when chain_source_type = "esplora")
    pub esplora_url: Option<String>,
    /// Number of parallel Esplora requests during wallet sync.
    ///
    /// Public Esplora servers often rate-limit bursty clients, so the default
    /// is conservative. Increase only when using a private or higher-limit
    /// Esplora server.
    #[serde(default = "default_bdk_esplora_parallel_requests")]
    pub esplora_parallel_requests: usize,
    /// Electrum URL (when chain_source_type = "electrum")
    pub electrum_url: Option<String>,
    /// Number of scripts to request in each Electrum sync batch
    #[serde(default = "default_bdk_electrum_batch_size")]
    pub electrum_batch_size: usize,
    /// Bitcoin RPC host (when chain_source_type = "bitcoinrpc")
    pub bitcoind_rpc_host: Option<String>,
    /// Bitcoin RPC port
    pub bitcoind_rpc_port: Option<u16>,
    /// Bitcoin RPC user
    pub bitcoind_rpc_user: Option<String>,
    /// Bitcoin RPC password
    pub bitcoind_rpc_password: Option<String>,
    /// Optional birthday height used when creating a fresh Bitcoin RPC wallet.
    ///
    /// If unset, the wallet starts at the current chain tip. Set this when
    /// restoring from a mnemonic to rescan from a known height. Existing
    /// wallets are not rewound.
    pub wallet_rescan_from_height: Option<u32>,
    /// BIP-39 mnemonic for the BDK wallet
    pub mnemonic: Option<String>,
    /// Batch processor configuration
    #[serde(default)]
    pub batch_config: BatchConfig,
    /// Number of confirmations required for incoming payments.
    ///
    /// Must be >= 1. A value of 0 is rejected at startup because the
    /// confirmation check still requires the transaction to have an on-chain
    /// anchor (i.e. 0 would mean "confirmed in any block", not "accept
    /// unconfirmed"). Use 1 for "accept any confirmation".
    #[serde(default = "default_bdk_num_confs")]
    pub num_confs: u32,
    /// Minimum receive amount in sats
    #[serde(default = "default_bdk_min_receive_amount_sat")]
    pub min_receive_amount_sat: u64,
    /// Minimum send amount in sats
    #[serde(default = "default_bdk_min_send_amount_sat")]
    pub min_send_amount_sat: u64,
    /// Wallet sync interval in seconds
    #[serde(default = "default_bdk_sync_interval_secs")]
    pub sync_interval_secs: u64,
}

#[cfg(feature = "bdk")]
impl fmt::Debug for Bdk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bdk")
            .field("fee_percent", &self.fee_percent)
            .field("reserve_fee_min", &self.reserve_fee_min)
            .field("network", &self.network)
            .field("chain_source_type", &self.chain_source_type)
            .field(
                "esplora_url",
                &self.esplora_url.as_deref().map(url_for_logs),
            )
            .field("esplora_parallel_requests", &self.esplora_parallel_requests)
            .field(
                "electrum_url",
                &self.electrum_url.as_deref().map(url_for_logs),
            )
            .field("electrum_batch_size", &self.electrum_batch_size)
            .field("bitcoind_rpc_host", &self.bitcoind_rpc_host)
            .field("bitcoind_rpc_port", &self.bitcoind_rpc_port)
            .field("bitcoind_rpc_user", &self.bitcoind_rpc_user)
            .field("bitcoind_rpc_password", &"[REDACTED]")
            .field("wallet_rescan_from_height", &self.wallet_rescan_from_height)
            .field("mnemonic", &"[REDACTED]")
            .field("batch_config", &self.batch_config)
            .field("num_confs", &self.num_confs)
            .field("min_receive_amount_sat", &self.min_receive_amount_sat)
            .field("min_send_amount_sat", &self.min_send_amount_sat)
            .field("sync_interval_secs", &self.sync_interval_secs)
            .finish()
    }
}

#[cfg(feature = "bdk")]
impl Default for Bdk {
    fn default() -> Self {
        Self {
            fee_percent: default_fee_percent(),
            reserve_fee_min: default_reserve_fee_min(),
            network: None,
            chain_source_type: None,
            esplora_url: None,
            esplora_parallel_requests: default_bdk_esplora_parallel_requests(),
            electrum_url: None,
            electrum_batch_size: default_bdk_electrum_batch_size(),
            bitcoind_rpc_host: None,
            bitcoind_rpc_port: None,
            bitcoind_rpc_user: None,
            bitcoind_rpc_password: None,
            wallet_rescan_from_height: None,
            mnemonic: None,
            batch_config: BatchConfig::default(),
            num_confs: default_bdk_num_confs(),
            min_receive_amount_sat: default_bdk_min_receive_amount_sat(),
            min_send_amount_sat: default_bdk_min_send_amount_sat(),
            sync_interval_secs: default_bdk_sync_interval_secs(),
        }
    }
}

#[cfg(feature = "bdk")]
impl Bdk {
    /// Validate BDK settings that must be rejected before the backend starts.
    pub fn validate(&self) -> Result<(), String> {
        if self.num_confs == 0 {
            return Err(
                "BDK num_confs must be >= 1 (0 is rejected because it still \
                 requires an on-chain anchor and is almost never intended; \
                 use 1 for 'any confirmation')"
                    .to_string(),
            );
        }

        if self.min_send_amount_sat == 0 {
            return Err("BDK min_send_amount_sat must be >= 1".to_string());
        }

        if self.batch_config.target_block_time_secs == 0 {
            return Err("BDK batch_config.target_block_time_secs must be >= 1".to_string());
        }

        validate_bdk_fee_options(&self.batch_config.fee_options)?;

        Ok(())
    }
}

#[cfg(feature = "bdk")]
fn default_bdk_num_confs() -> u32 {
    6
}

#[cfg(feature = "bdk")]
fn default_bdk_min_receive_amount_sat() -> u64 {
    1000
}

#[cfg(feature = "bdk")]
fn default_bdk_min_send_amount_sat() -> u64 {
    546
}

#[cfg(feature = "bdk")]
fn default_bdk_sync_interval_secs() -> u64 {
    30
}

#[cfg(feature = "bdk")]
fn default_bdk_esplora_parallel_requests() -> usize {
    1
}

#[cfg(feature = "bdk")]
fn default_bdk_electrum_batch_size() -> usize {
    5
}

#[cfg(feature = "bdk")]
fn default_bdk_poll_interval_secs() -> u64 {
    30
}

#[cfg(feature = "bdk")]
fn default_bdk_max_batch_size() -> usize {
    50
}

#[cfg(feature = "bdk")]
fn default_bdk_target_block_time_secs() -> u64 {
    cdk_bdk::DEFAULT_TARGET_BLOCK_TIME_SECS
}

#[cfg(feature = "bdk")]
fn default_bdk_fee_options() -> Vec<String> {
    vec!["immediate".to_string()]
}

#[cfg(feature = "bdk")]
fn validate_bdk_fee_options(fee_options: &[String]) -> Result<(), String> {
    let tiers = fee_options
        .iter()
        .map(|tier| {
            cdk_bdk::PaymentTier::from_config_name(tier).ok_or_else(|| {
                format!(
                    "Unknown BDK batch_config.fee_options tier '{tier}'; expected immediate, standard, or economy"
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    cdk_bdk::types::validate_fee_options(&tiers)
}

#[cfg(feature = "bdk")]
fn default_bdk_fee_fallback_sat_per_vb() -> f64 {
    2.0
}

#[cfg(feature = "bdk")]
fn default_bdk_fee_cache_ttl_secs() -> u64 {
    60
}

#[cfg(feature = "bdk")]
fn default_bdk_quote_max_input_count() -> usize {
    24
}

#[cfg(feature = "bdk")]
fn default_bdk_quote_fixed_safety_sat() -> u64 {
    500
}

#[cfg(feature = "bdk")]
fn default_bdk_quote_safety_multiplier() -> f64 {
    1.25
}

#[cfg(feature = "cln")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Cln {
    pub rpc_path: PathBuf,
    #[serde(default = "default_cln_bolt12")]
    pub bolt12: bool,
    #[serde(default)]
    pub expose_private_channels: bool,
    #[serde(default = "default_fee_percent")]
    pub fee_percent: f32,
    #[serde(default = "default_reserve_fee_min")]
    pub reserve_fee_min: Amount,
}

#[cfg(feature = "cln")]
impl Default for Cln {
    fn default() -> Self {
        Self {
            rpc_path: PathBuf::new(),
            bolt12: true,
            expose_private_channels: false,
            fee_percent: 0.02,
            reserve_fee_min: 2.into(),
        }
    }
}

#[cfg(feature = "cln")]
fn default_cln_bolt12() -> bool {
    true
}

#[cfg(feature = "lnd")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lnd {
    pub address: String,
    pub cert_file: PathBuf,
    pub macaroon_file: PathBuf,
    #[serde(default = "default_fee_percent")]
    pub fee_percent: f32,
    #[serde(default = "default_reserve_fee_min")]
    pub reserve_fee_min: Amount,
}

#[cfg(feature = "lnd")]
impl Default for Lnd {
    fn default() -> Self {
        Self {
            address: String::new(),
            cert_file: PathBuf::new(),
            macaroon_file: PathBuf::new(),
            fee_percent: 0.02,
            reserve_fee_min: 2.into(),
        }
    }
}

#[cfg(feature = "ldk-node")]
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LdkNode {
    /// Fee percentage (e.g., 0.02 for 2%)
    #[serde(default = "default_ldk_fee_percent")]
    pub fee_percent: f32,
    /// Minimum reserve fee
    #[serde(default = "default_ldk_reserve_fee_min")]
    pub reserve_fee_min: Amount,
    /// Bitcoin network (mainnet, testnet, signet, regtest)
    pub bitcoin_network: Option<String>,
    /// Chain source type (esplora, electrum, or bitcoinrpc)
    pub chain_source_type: Option<String>,
    /// Esplora URL (when chain_source_type = "esplora")
    pub esplora_url: Option<String>,
    /// Electrum URL (when chain_source_type = "electrum")
    pub electrum_url: Option<String>,
    /// Bitcoin RPC configuration (when chain_source_type = "bitcoinrpc")
    pub bitcoind_rpc_host: Option<String>,
    pub bitcoind_rpc_port: Option<u16>,
    pub bitcoind_rpc_user: Option<String>,
    pub bitcoind_rpc_password: Option<String>,
    /// Storage directory path
    pub storage_dir_path: Option<String>,
    /// Log directory path (logging stdout if omitted)
    pub log_dir_path: Option<String>,
    /// LDK node listening host
    pub ldk_node_host: Option<String>,
    /// LDK node listening port
    pub ldk_node_port: Option<u16>,
    /// LDK node announcement addresses
    pub ldk_node_announce_addresses: Option<Vec<String>>,
    /// Gossip source type (p2p or rgs)
    pub gossip_source_type: Option<String>,
    /// Rapid Gossip Sync URL (when gossip_source_type = "rgs")
    pub rgs_url: Option<String>,
    /// Webserver host (defaults to 127.0.0.1)
    #[serde(default = "default_webserver_host")]
    pub webserver_host: Option<String>,
    /// Webserver port
    #[serde(default = "default_webserver_port")]
    pub webserver_port: Option<u16>,
    /// LDK node mnemonic
    /// If not set, LDK node will use its default seed storage mechanism
    pub ldk_node_mnemonic: Option<String>,
}

#[cfg(feature = "ldk-node")]
impl Default for LdkNode {
    fn default() -> Self {
        Self {
            fee_percent: default_ldk_fee_percent(),
            reserve_fee_min: default_ldk_reserve_fee_min(),
            bitcoin_network: None,
            chain_source_type: None,
            esplora_url: None,
            electrum_url: None,
            bitcoind_rpc_host: None,
            bitcoind_rpc_port: None,
            bitcoind_rpc_user: None,
            ldk_node_announce_addresses: None,
            bitcoind_rpc_password: None,
            storage_dir_path: None,
            ldk_node_host: None,
            log_dir_path: None,
            ldk_node_port: None,
            gossip_source_type: None,
            rgs_url: None,
            webserver_host: default_webserver_host(),
            webserver_port: default_webserver_port(),
            ldk_node_mnemonic: None,
        }
    }
}

#[cfg(feature = "ldk-node")]
impl fmt::Debug for LdkNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LdkNode")
            .field("fee_percent", &self.fee_percent)
            .field("reserve_fee_min", &self.reserve_fee_min)
            .field("bitcoin_network", &self.bitcoin_network)
            .field("chain_source_type", &self.chain_source_type)
            .field(
                "esplora_url",
                &self.esplora_url.as_deref().map(url_for_logs),
            )
            .field(
                "electrum_url",
                &self.electrum_url.as_deref().map(url_for_logs),
            )
            .field("bitcoind_rpc_host", &self.bitcoind_rpc_host)
            .field("bitcoind_rpc_port", &self.bitcoind_rpc_port)
            .field("bitcoind_rpc_user", &self.bitcoind_rpc_user)
            .field("bitcoind_rpc_password", &"[REDACTED]")
            .field("storage_dir_path", &self.storage_dir_path)
            .field("log_dir_path", &self.log_dir_path)
            .field("ldk_node_host", &self.ldk_node_host)
            .field("ldk_node_port", &self.ldk_node_port)
            .field(
                "ldk_node_announce_addresses",
                &self.ldk_node_announce_addresses,
            )
            .field("gossip_source_type", &self.gossip_source_type)
            .field("rgs_url", &self.rgs_url.as_deref().map(url_for_logs))
            .field("webserver_host", &self.webserver_host)
            .field("webserver_port", &self.webserver_port)
            .field("ldk_node_mnemonic", &"[REDACTED]")
            .finish()
    }
}

#[cfg(feature = "ldk-node")]
fn default_ldk_fee_percent() -> f32 {
    0.04
}

#[cfg(feature = "ldk-node")]
fn default_ldk_reserve_fee_min() -> Amount {
    4.into()
}

#[cfg(feature = "ldk-node")]
fn default_webserver_host() -> Option<String> {
    Some("127.0.0.1".to_string())
}

#[cfg(feature = "ldk-node")]
fn default_webserver_port() -> Option<u16> {
    Some(8091)
}

#[cfg(feature = "fakewallet")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeWalletKeysetRotation {
    /// Currency unit (e.g. "sat", "usd")
    pub unit: CurrencyUnit,
    /// Input fee in parts per thousand
    #[serde(default)]
    pub input_fee_ppk: u64,
    /// Keyset version: "v1" (Version00) or "v2" (Version01)
    #[serde(default = "default_keyset_version")]
    pub version: String,
}

#[cfg(feature = "fakewallet")]
fn default_keyset_version() -> String {
    "v1".to_string()
}

#[cfg(feature = "fakewallet")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FakeWalletCustomPaymentMethod {
    /// Custom method available for every supported fake wallet unit
    Method(String),
    /// Custom method available only for one unit
    MethodForUnit {
        /// Payment method name (e.g. "paypal", "venmo")
        method: String,
        /// Currency unit for this method
        unit: CurrencyUnit,
    },
}

#[cfg(feature = "fakewallet")]
impl FakeWalletCustomPaymentMethod {
    pub fn method(&self) -> &str {
        match self {
            Self::Method(method) => method,
            Self::MethodForUnit { method, .. } => method,
        }
    }

    pub fn applies_to_unit(&self, unit: &CurrencyUnit) -> bool {
        match self {
            Self::Method(_) => true,
            Self::MethodForUnit {
                unit: method_unit, ..
            } => method_unit == unit,
        }
    }
}

#[cfg(feature = "fakewallet")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FakeWallet {
    #[serde(default = "default_fake_wallet_supported_units")]
    pub supported_units: Vec<CurrencyUnit>,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
    #[serde(default = "default_fake_wallet_custom_payment_methods")]
    pub custom_payment_methods: Vec<FakeWalletCustomPaymentMethod>,
    #[serde(default = "default_min_delay_time")]
    pub min_delay_time: u64,
    #[serde(default = "default_max_delay_time")]
    pub max_delay_time: u64,
    /// Additional keyset rotations to create during mint build
    #[serde(default)]
    pub keyset_rotations: Vec<FakeWalletKeysetRotation>,
}

#[cfg(feature = "fakewallet")]
impl Default for FakeWallet {
    fn default() -> Self {
        Self {
            supported_units: vec![CurrencyUnit::Sat],
            fee_percent: 0.02,
            reserve_fee_min: 2.into(),
            custom_payment_methods: default_fake_wallet_custom_payment_methods(),
            min_delay_time: 1,
            max_delay_time: 3,
            keyset_rotations: Vec::new(),
        }
    }
}

// Helper functions to provide default values
// Common fee defaults for all backends
#[cfg(any(feature = "cln", feature = "lnd"))]
fn default_fee_percent() -> f32 {
    0.02
}

#[cfg(any(feature = "cln", feature = "lnd"))]
fn default_reserve_fee_min() -> Amount {
    2.into()
}

#[cfg(feature = "fakewallet")]
fn default_min_delay_time() -> u64 {
    1
}

#[cfg(feature = "fakewallet")]
fn default_max_delay_time() -> u64 {
    3
}

#[cfg(feature = "fakewallet")]
fn default_fake_wallet_custom_payment_methods() -> Vec<FakeWalletCustomPaymentMethod> {
    vec![FakeWalletCustomPaymentMethod::Method("paypal".to_string())]
}

#[cfg(feature = "fakewallet")]
fn default_fake_wallet_supported_units() -> Vec<CurrencyUnit> {
    vec![CurrencyUnit::Sat]
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct GrpcProcessor {
    #[serde(default)]
    pub supported_units: Vec<CurrencyUnit>,
    #[serde(default = "default_grpc_address", alias = "addr")]
    pub address: String,
    #[serde(default = "default_grpc_port")]
    pub port: u16,
    #[serde(default)]
    pub tls_dir: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure: bool,
}

impl Default for GrpcProcessor {
    fn default() -> Self {
        Self {
            supported_units: Vec::new(),
            address: default_grpc_address(),
            port: default_grpc_port(),
            tls_dir: None,
            allow_insecure: false,
        }
    }
}

fn default_grpc_address() -> String {
    "127.0.0.1".to_string()
}

fn default_grpc_port() -> u16 {
    50051
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseEngine {
    #[default]
    Sqlite,
    Postgres,
}

impl std::str::FromStr for DatabaseEngine {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sqlite" => Ok(DatabaseEngine::Sqlite),
            "postgres" => Ok(DatabaseEngine::Postgres),
            _ => Err(format!("Unknown database engine: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Database {
    pub engine: DatabaseEngine,
    pub postgres: Option<PostgresConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthDatabase {
    pub postgres: Option<PostgresAuthConfig>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PostgresAuthConfig {
    pub url: String,
    pub tls_mode: Option<String>,
    pub max_connections: Option<usize>,
    pub connection_timeout_seconds: Option<u64>,
}

impl fmt::Debug for PostgresAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresAuthConfig")
            .field("url", &url_for_logs(&self.url))
            .field("tls_mode", &self.tls_mode)
            .field("max_connections", &self.max_connections)
            .field(
                "connection_timeout_seconds",
                &self.connection_timeout_seconds,
            )
            .finish()
    }
}

impl Default for PostgresAuthConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            tls_mode: Some("disable".to_string()),
            max_connections: Some(20),
            connection_timeout_seconds: Some(10),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PostgresConfig {
    pub url: String,
    pub tls_mode: Option<String>,
    pub max_connections: Option<usize>,
    pub connection_timeout_seconds: Option<u64>,
}

impl fmt::Debug for PostgresConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresConfig")
            .field("url", &url_for_logs(&self.url))
            .field("tls_mode", &self.tls_mode)
            .field("max_connections", &self.max_connections)
            .field(
                "connection_timeout_seconds",
                &self.connection_timeout_seconds,
            )
            .finish()
    }
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            tls_mode: Some("disable".to_string()),
            max_connections: Some(20),
            connection_timeout_seconds: Some(10),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuthType {
    Clear,
    Blind,
    #[default]
    None,
}

impl std::str::FromStr for AuthType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "clear" => Ok(AuthType::Clear),
            "blind" => Ok(AuthType::Blind),
            "none" => Ok(AuthType::None),
            _ => Err(format!("Unknown auth type: {s}")),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Auth {
    #[serde(default)]
    pub auth_enabled: bool,
    pub openid_discovery: String,
    pub openid_client_id: String,
    pub mint_max_bat: u64,
    #[serde(default = "default_blind")]
    pub mint: AuthType,
    #[serde(default)]
    pub get_mint_quote: AuthType,
    #[serde(default)]
    pub check_mint_quote: AuthType,
    #[serde(default)]
    pub melt: AuthType,
    #[serde(default)]
    pub get_melt_quote: AuthType,
    #[serde(default)]
    pub check_melt_quote: AuthType,
    #[serde(default = "default_blind")]
    pub swap: AuthType,
    #[serde(default = "default_blind")]
    pub restore: AuthType,
    #[serde(default)]
    pub check_proof_state: AuthType,
    /// Enable WebSocket authentication support
    #[serde(default = "default_blind")]
    pub websocket_auth: AuthType,
}

fn default_blind() -> AuthType {
    AuthType::Blind
}

/// CDK settings, derived from `config.toml`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub info: Info,
    pub signatory: Option<Signatory>,
    pub mint_info: MintInfo,
    #[serde(default, deserialize_with = "deserialize_payment_backend")]
    pub payment_backend: Vec<PaymentBackend>,
    pub onchain: Option<Onchain>,
    /// Transaction limits for DoS protection
    #[serde(default)]
    pub limits: Limits,
    #[cfg(feature = "cln")]
    pub cln: Option<Cln>,
    #[cfg(feature = "lnd")]
    pub lnd: Option<Lnd>,
    #[cfg(feature = "ldk-node")]
    pub ldk_node: Option<LdkNode>,
    #[cfg(feature = "fakewallet")]
    pub fake_wallet: Option<FakeWallet>,
    pub grpc_processor: Option<GrpcProcessor>,
    #[cfg(feature = "bdk")]
    pub bdk: Option<Bdk>,
    pub database: Database,
    pub auth_database: Option<AuthDatabase>,
    #[cfg(feature = "management-rpc")]
    pub mint_management_rpc: Option<MintManagementRpc>,
    pub auth: Option<Auth>,
    #[cfg(feature = "prometheus")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prometheus: Option<Prometheus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg(feature = "prometheus")]
#[serde(default)]
pub struct Prometheus {
    pub enabled: bool,
    pub address: Option<String>,
    pub port: Option<u16>,
}

/// Transaction limits configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum number of inputs allowed per transaction (swap/melt)
    #[serde(default = "default_max_inputs")]
    pub max_inputs: usize,
    /// Maximum number of outputs allowed per transaction (mint/swap/melt)
    #[serde(default = "default_max_outputs")]
    pub max_outputs: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_inputs: 1000,
            max_outputs: 1000,
        }
    }
}

fn default_max_inputs() -> usize {
    1000
}

fn default_max_outputs() -> usize {
    1000
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MintInfo {
    /// name of the mint and should be recognizable
    pub name: String,
    /// hex pubkey of the mint
    pub pubkey: Option<PublicKey>,
    /// short description of the mint
    pub description: String,
    /// long description
    pub description_long: Option<String>,
    /// url to the mint icon
    pub icon_url: Option<String>,
    /// message of the day that the wallet must display to the user
    pub motd: Option<String>,
    /// Nostr publickey
    pub contact_nostr_public_key: Option<String>,
    /// Contact email
    pub contact_email: Option<String>,
    /// URL to the terms of service
    pub tos_url: Option<String>,
}

#[cfg(feature = "management-rpc")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MintManagementRpc {
    /// When this is set to `true` the mint use the config file for the initial set up on first start.
    /// Changes to the `[mint_info]` after this **MUST** be made via the RPC changes to the config file or env vars will be ignored.
    pub enabled: bool,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub tls_dir: Option<PathBuf>,
    #[serde(default)]
    pub allow_insecure: bool,
}

impl Settings {
    /// Parses settings from an in-memory TOML import document.
    pub fn try_from_toml(document: &str) -> Result<Self, ConfigError> {
        Self::try_from_toml_allowing(document, &[])
    }

    /// Parses settings while allowing a narrow set of fields owned by the
    /// legacy migration layer.
    pub(crate) fn try_from_toml_allowing(
        document: &str,
        allowed_unknown_fields: &[&str],
    ) -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let configuration = Config::builder()
            .add_source(Config::try_from(&defaults)?)
            .add_source(File::from_str(document, FileFormat::Toml))
            .build()?;
        Self::deserialize_configuration(configuration, allowed_unknown_fields)
    }

    fn deserialize_configuration(
        configuration: Config,
        allowed_unknown_fields: &[&str],
    ) -> Result<Self, ConfigError> {
        let mut unknown_fields = Vec::new();
        let settings = serde_ignored::deserialize(configuration, |path| {
            let path = path.to_string().replace(".?.", ".");
            if !allowed_unknown_fields.contains(&path.as_str()) {
                unknown_fields.push(path);
            }
        })?;
        unknown_fields.sort();
        unknown_fields.dedup();
        if unknown_fields.is_empty() {
            Ok(settings)
        } else {
            let mut message = format!(
                "unknown configuration field(s): {}",
                unknown_fields.join(", ")
            );
            if unknown_fields.iter().any(|field| field == "ln") {
                message.push_str(
                    "; legacy `[ln]`/`[[ln]]` configuration must be migrated to \
                     `[payment_backend]`/`[[payment_backend]]`; run `cdk-mintd config migrate \
                     --file <old> --output <new>`",
                );
            }
            Err(ConfigError::Message(message))
        }
    }

    pub fn validate_backend_pairing(&self) -> Result<(), String> {
        #[cfg(feature = "fakewallet")]
        self.validate_fake_wallet_backend_pairing()?;

        Ok(())
    }

    #[cfg(feature = "fakewallet")]
    fn validate_fake_wallet_backend_pairing(&self) -> Result<(), String> {
        let onchain_backend = self
            .onchain
            .as_ref()
            .map(|onchain| &onchain.onchain_backend)
            .unwrap_or(&OnchainBackend::None);

        let has_fake_wallet_backend = self
            .payment_backend
            .iter()
            .any(|backend| backend.backend == PaymentBackendType::FakeWallet);
        let has_real_backend = self.payment_backend.iter().any(|backend| {
            !matches!(
                backend.backend,
                PaymentBackendType::None | PaymentBackendType::FakeWallet
            )
        });

        if has_fake_wallet_backend && has_real_backend {
            return Err("backend = \"fakewallet\" cannot be combined with a real \
                 payment backend; use only fakewallet backends or only real backends"
                .to_string());
        }

        match onchain_backend {
            #[cfg(feature = "bdk")]
            OnchainBackend::Bdk if has_fake_wallet_backend => {
                return Err("backend = \"fakewallet\" cannot be combined with \
                     onchain_backend = \"bdk\"; use onchain_backend = \
                     \"fakewallet\" or \"none\""
                    .to_string());
            }
            OnchainBackend::FakeWallet if has_real_backend => {
                return Err("onchain_backend = \"fakewallet\" cannot be combined with \
                     a real payment backend; use backend = \"fakewallet\" \
                     or \"none\""
                    .to_string());
            }
            _ => {}
        }

        Ok(())
    }

    pub fn try_new<P>(config_file_name: Option<P>) -> Result<Self, ConfigError>
    where
        P: Into<PathBuf>,
    {
        let default_settings = Self::default();
        Self::new_from_default(&default_settings, config_file_name)
    }

    /// Loads settings from defaults and an optional config file.
    ///
    /// New code should use [`Self::try_new`] so configuration errors can be
    /// reported to the caller. This method retains its historical fallback
    /// behavior for API compatibility.
    #[deprecated(note = "use Settings::try_new to handle configuration errors")]
    #[must_use]
    pub fn new<P>(config_file_name: Option<P>) -> Self
    where
        P: Into<PathBuf>,
    {
        match Self::try_new(config_file_name) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::error!("Error reading config file, falling back to defaults: {err}");
                Self::default()
            }
        }
    }

    fn new_from_default<P>(
        default: &Settings,
        config_file_name: Option<P>,
    ) -> Result<Self, ConfigError>
    where
        P: Into<PathBuf>,
    {
        let mut default_config_file_name = home::home_dir()
            .ok_or(ConfigError::NotFound("Config Path".to_string()))?
            .join("cashu-rs-mint");

        default_config_file_name.push("config.toml");
        let config: String = match config_file_name {
            Some(value) => value.into().to_string_lossy().to_string(),
            None => default_config_file_name.to_string_lossy().to_string(),
        };
        let builder = Config::builder();
        let config: Config = builder
            // use defaults
            .add_source(Config::try_from(default)?)
            // override with file contents
            .add_source(File::with_name(&config))
            .build()?;
        Self::deserialize_configuration(config, &[])
    }

    pub(crate) fn enabled_signatory(&self) -> Option<&Signatory> {
        self.signatory
            .as_ref()
            .filter(|signatory| signatory.enabled)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn postgres_config_debug_redacts_connection_credentials() {
        let secret = "postgres-password-secret";
        let url = format!("postgres://mint:{secret}@db.example.com/cdk?token={secret}");
        let database = Database {
            engine: DatabaseEngine::Postgres,
            postgres: Some(PostgresConfig {
                url: url.clone(),
                ..Default::default()
            }),
        };
        let auth_database = AuthDatabase {
            postgres: Some(PostgresAuthConfig {
                url,
                ..Default::default()
            }),
        };

        let database_debug = format!("{database:?}");
        let auth_debug = format!("{auth_database:?}");

        assert!(database_debug.contains("postgres://db.example.com/cdk"));
        assert!(auth_debug.contains("postgres://db.example.com/cdk"));
        assert!(!database_debug.contains(secret));
        assert!(!auth_debug.contains(secret));
    }

    #[cfg(feature = "ldk-node")]
    #[test]
    fn ldk_node_backend_accepts_supported_spellings() {
        for backend in ["ldk-node", "ldknode"] {
            let document = format!(
                r#"
[[payment_backend]]
backend = "{backend}"
unit = "sat"
"#
            );

            let settings =
                Settings::try_from_toml(&document).expect("LDK Node backend should deserialize");

            assert_eq!(
                settings
                    .payment_backend
                    .first()
                    .expect("config should contain one payment backend")
                    .backend,
                PaymentBackendType::LdkNode
            );
        }
    }

    #[test]
    fn toml_parser_rejects_unknown_fields_with_full_paths() {
        let error = Settings::try_from_toml(
            r#"
[info]
listen_por = 8085

[database]
engin = "sqlite"
"#,
        )
        .expect_err("misspelled fields must be rejected");
        let message = error.to_string();
        assert!(message.contains("database.engin"));
        assert!(message.contains("info.listen_por"));
    }

    #[test]
    fn toml_parser_reports_legacy_ln_migration() {
        let error = Settings::try_from_toml(
            r#"
[ln]
ln_backend = "fakewallet"
"#,
        )
        .expect_err("legacy [ln] configuration must be rejected");
        let message = error.to_string();
        assert!(message.contains("legacy `[ln]`/`[[ln]]` configuration must be migrated"));
        assert!(message.contains("`[payment_backend]`/`[[payment_backend]]`"));
        assert!(message.contains("`cdk-mintd config migrate --file <old> --output <new>`"));
    }

    #[test]
    fn legacy_parser_allowlist_does_not_hide_unrelated_typos() {
        let error = Settings::try_from_toml_allowing(
            r#"
[info]
signatory_url = "http://127.0.0.1:10009"
listen_por = 8085
"#,
            &["info.signatory_url"],
        )
        .expect_err("only exact legacy fields may be ignored");
        let message = error.to_string();
        assert!(message.contains("info.listen_por"));
        assert!(!message.contains("info.signatory_url"));
    }

    fn config_env_lock() -> std::sync::MutexGuard<'static, ()> {
        // Share the single process-wide env lock with the rest of the crate's
        // tests. `std::env` is global, so config.rs and lib.rs tests must
        // serialize on the *same* mutex or they race over env vars.
        crate::test_utils::env_lock()
    }

    #[cfg(feature = "bdk")]
    fn clear_bdk_env_vars() {
        std::env::remove_var(crate::env_vars::BDK_MNEMONIC_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_NETWORK_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_CHAIN_SOURCE_TYPE_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_ESPLORA_URL_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_ELECTRUM_URL_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_ELECTRUM_BATCH_SIZE_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_WALLET_RESCAN_FROM_HEIGHT_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_MIN_SEND_AMOUNT_SAT_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_TARGET_BLOCK_TIME_SECS_ENV_VAR);
        std::env::remove_var(crate::env_vars::BDK_FEE_OPTIONS_ENV_VAR);
        std::env::remove_var(crate::env_vars::ENV_ONCHAIN_BACKEND);
    }

    #[test]
    fn test_info_debug_impl() {
        // Create a sample Info struct with test data
        let info = Info {
            url: "http://example.com".to_string(),
            listen_host: "127.0.0.1".to_string(),
            listen_port: 8080,
            mnemonic: Some("test secret mnemonic phrase".to_string()),
            input_fee_ppk: Some(100),
            ..Default::default()
        };

        // Convert the Info struct to a debug string
        let debug_output = format!("{info:?}");

        // Verify the debug output contains expected fields
        assert!(debug_output.contains("url: \"http://example.com\""));
        assert!(debug_output.contains("listen_host: \"127.0.0.1\""));
        assert!(debug_output.contains("listen_port: 8080"));

        // The mnemonic should be hashed, not displayed in plaintext
        assert!(!debug_output.contains("test secret mnemonic phrase"));
        assert!(debug_output.contains("<hashed: "));

        assert!(debug_output.contains("input_fee_ppk: Some(100)"));
    }

    #[test]
    fn test_info_debug_with_empty_mnemonic() {
        // Test with an empty mnemonic to ensure it doesn't panic
        let info = Info {
            url: "http://example.com".to_string(),
            listen_host: "127.0.0.1".to_string(),
            listen_port: 8080,
            mnemonic: Some("".to_string()), // Empty mnemonic
            ..Default::default()
        };

        // This should not panic
        let debug_output = format!("{:?}", info);

        // The empty mnemonic should still be hashed
        assert!(debug_output.contains("<hashed: "));
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_default_min_send_amount_sat() {
        assert_eq!(Bdk::default().min_send_amount_sat, 546);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_default_electrum_batch_size() {
        assert_eq!(Bdk::default().electrum_batch_size, 5);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_debug_redacts_secrets_and_url_credentials() {
        let config = Bdk {
            network: Some("regtest".to_string()),
            esplora_url: Some(
                "https://esplora-user:esplora-secret@example.com/esplora".to_string(),
            ),
            electrum_url: Some("ssl://electrum-user:electrum-secret@example.com:50002".to_string()),
            bitcoind_rpc_password: Some("rpc-password-secret".to_string()),
            mnemonic: Some("mnemonic-secret-words".to_string()),
            ..Default::default()
        };

        let debug = format!("{config:?}");

        assert!(debug.contains("regtest"));
        assert!(debug.contains("https://example.com/esplora"));
        assert!(debug.contains("ssl://example.com:50002"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("esplora-user"));
        assert!(!debug.contains("esplora-secret"));
        assert!(!debug.contains("electrum-user"));
        assert!(!debug.contains("electrum-secret"));
        assert!(!debug.contains("rpc-password-secret"));
        assert!(!debug.contains("mnemonic-secret-words"));
    }

    #[cfg(feature = "ldk-node")]
    #[test]
    fn test_ldk_node_debug_redacts_secrets_and_url_credentials() {
        let config = LdkNode {
            bitcoin_network: Some("regtest".to_string()),
            esplora_url: Some(
                "https://esplora-user:esplora-secret@example.com/esplora".to_string(),
            ),
            electrum_url: Some("ssl://electrum-user:electrum-secret@example.com:50002".to_string()),
            bitcoind_rpc_password: Some("rpc-password-secret".to_string()),
            rgs_url: Some("https://rgs-user:rgs-secret@example.com/snapshot".to_string()),
            ldk_node_mnemonic: Some("mnemonic-secret-words".to_string()),
            ..Default::default()
        };

        let debug = format!("{config:?}");

        assert!(debug.contains("regtest"));
        assert!(debug.contains("https://example.com/esplora"));
        assert!(debug.contains("ssl://example.com:50002"));
        assert!(debug.contains("https://example.com/snapshot"));
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("esplora-user"));
        assert!(!debug.contains("esplora-secret"));
        assert!(!debug.contains("electrum-user"));
        assert!(!debug.contains("electrum-secret"));
        assert!(!debug.contains("rgs-user"));
        assert!(!debug.contains("rgs-secret"));
        assert!(!debug.contains("rpc-password-secret"));
        assert!(!debug.contains("mnemonic-secret-words"));
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_electrum_toml_config() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_bdk_electrum_config");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[bdk]
network = "regtest"
chain_source_type = "electrum"
electrum_url = "tcp://127.0.0.1:50001"
electrum_batch_size = 11
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path))
            .expect("electrum config should parse successfully");
        let bdk = settings.bdk.expect("bdk config should be present");

        assert_eq!(bdk.chain_source_type, Some("electrum".to_string()));
        assert_eq!(bdk.electrum_url, Some("tcp://127.0.0.1:50001".to_string()));
        assert_eq!(bdk.electrum_batch_size, 11);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_config_min_send_amount_sat_override() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_bdk_min_send_config");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[bdk]
min_send_amount_sat = 1200
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should load");

        assert_eq!(
            settings
                .bdk
                .as_ref()
                .expect("bdk config should be present")
                .min_send_amount_sat,
            1200
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_env_min_send_amount_sat_override() {
        let _guard = config_env_lock();
        clear_bdk_env_vars();
        std::env::set_var(crate::env_vars::ENV_ONCHAIN_BACKEND, "bdk");
        std::env::set_var(crate::env_vars::BDK_NETWORK_ENV_VAR, "regtest");
        std::env::set_var(crate::env_vars::BDK_MIN_SEND_AMOUNT_SAT_ENV_VAR, "777");

        let mut settings = Settings::default();
        settings.from_env().expect("Failed to apply env vars");

        assert_eq!(
            settings
                .bdk
                .as_ref()
                .expect("bdk config should be present")
                .min_send_amount_sat,
            777
        );

        clear_bdk_env_vars();
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_env_wallet_rescan_height_override() {
        let _guard = config_env_lock();
        clear_bdk_env_vars();
        std::env::set_var(crate::env_vars::ENV_ONCHAIN_BACKEND, "bdk");
        std::env::set_var(crate::env_vars::BDK_NETWORK_ENV_VAR, "regtest");
        std::env::set_var(
            crate::env_vars::BDK_WALLET_RESCAN_FROM_HEIGHT_ENV_VAR,
            "850000",
        );

        let mut settings = Settings::default();
        settings.from_env().expect("Failed to apply env vars");

        assert_eq!(
            settings
                .bdk
                .as_ref()
                .expect("bdk config should be present")
                .wallet_rescan_from_height,
            Some(850000)
        );

        clear_bdk_env_vars();
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_env_electrum_config() {
        let _guard = config_env_lock();
        clear_bdk_env_vars();
        std::env::set_var(crate::env_vars::ENV_ONCHAIN_BACKEND, "bdk");
        std::env::set_var(crate::env_vars::BDK_NETWORK_ENV_VAR, "regtest");
        std::env::set_var(crate::env_vars::BDK_CHAIN_SOURCE_TYPE_ENV_VAR, "electrum");
        std::env::set_var(
            crate::env_vars::BDK_ELECTRUM_URL_ENV_VAR,
            "tcp://127.0.0.1:50001",
        );
        std::env::set_var(crate::env_vars::BDK_ELECTRUM_BATCH_SIZE_ENV_VAR, "9");

        let mut settings = Settings::default();
        settings.from_env().expect("Failed to apply env vars");

        let bdk = settings.bdk.expect("bdk config should be present");
        assert_eq!(bdk.chain_source_type, Some("electrum".to_string()));
        assert_eq!(bdk.electrum_url, Some("tcp://127.0.0.1:50001".to_string()));
        assert_eq!(bdk.electrum_batch_size, 9);

        clear_bdk_env_vars();
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_default_fee_options_immediate_only() {
        assert_eq!(
            Bdk::default().batch_config.fee_options,
            vec!["immediate".to_string()]
        );
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_default_batch_deadlines_derive_from_target_block_time() {
        let batch_config: cdk_bdk::BatchConfig = Bdk::default().batch_config.into();

        assert_eq!(
            batch_config.target_block_time,
            std::time::Duration::from_secs(600)
        );
        assert_eq!(
            batch_config.standard_deadline,
            std::time::Duration::from_secs(3600)
        );
        assert_eq!(
            batch_config.economy_deadline,
            std::time::Duration::from_secs(86_400)
        );
        assert_eq!(
            batch_config.max_intent_age,
            Some(std::time::Duration::from_secs(86_430))
        );
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_config_fee_options_override() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_bdk_fee_options_config");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[bdk.batch_config]
fee_options = ["immediate", "economy"]
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should load");

        assert_eq!(
            settings
                .bdk
                .as_ref()
                .expect("bdk config should be present")
                .batch_config
                .fee_options,
            vec!["immediate".to_string(), "economy".to_string()]
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_config_target_block_time_derives_deadlines() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_bdk_target_block_time_config");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[bdk.batch_config]
target_block_time_secs = 300
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should load");
        let batch_config: cdk_bdk::BatchConfig = settings
            .bdk
            .as_ref()
            .expect("bdk config should be present")
            .batch_config
            .clone()
            .into();

        assert_eq!(
            batch_config.target_block_time,
            std::time::Duration::from_secs(300)
        );
        assert_eq!(
            batch_config.standard_deadline,
            std::time::Duration::from_secs(1800)
        );
        assert_eq!(
            batch_config.economy_deadline,
            std::time::Duration::from_secs(43_200)
        );
        assert_eq!(
            batch_config.max_intent_age,
            Some(std::time::Duration::from_secs(43_230))
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_env_fee_options_override() {
        let _guard = config_env_lock();
        clear_bdk_env_vars();
        std::env::set_var(crate::env_vars::ENV_ONCHAIN_BACKEND, "bdk");
        std::env::set_var(crate::env_vars::BDK_NETWORK_ENV_VAR, "regtest");
        std::env::set_var(
            crate::env_vars::BDK_FEE_OPTIONS_ENV_VAR,
            "immediate,standard,economy",
        );

        let mut settings = Settings::default();
        settings.from_env().expect("Failed to apply env vars");

        assert_eq!(
            settings
                .bdk
                .as_ref()
                .expect("bdk config should be present")
                .batch_config
                .fee_options,
            vec![
                "immediate".to_string(),
                "standard".to_string(),
                "economy".to_string()
            ]
        );

        clear_bdk_env_vars();
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_env_target_block_time_override() {
        let _guard = config_env_lock();
        clear_bdk_env_vars();
        std::env::set_var(crate::env_vars::ENV_ONCHAIN_BACKEND, "bdk");
        std::env::set_var(crate::env_vars::BDK_NETWORK_ENV_VAR, "regtest");
        std::env::set_var(crate::env_vars::BDK_TARGET_BLOCK_TIME_SECS_ENV_VAR, "120");

        let mut settings = Settings::default();
        settings.from_env().expect("Failed to apply env vars");
        let batch_config: cdk_bdk::BatchConfig = settings
            .bdk
            .as_ref()
            .expect("bdk config should be present")
            .batch_config
            .clone()
            .into();

        assert_eq!(
            batch_config.target_block_time,
            std::time::Duration::from_secs(120)
        );
        assert_eq!(
            batch_config.standard_deadline,
            std::time::Duration::from_secs(720)
        );
        assert_eq!(
            batch_config.economy_deadline,
            std::time::Duration::from_secs(17_280)
        );
        assert_eq!(
            batch_config.max_intent_age,
            Some(std::time::Duration::from_secs(17_310))
        );

        clear_bdk_env_vars();
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_invalid_fee_options_rejected() {
        for fee_options in [
            Vec::new(),
            vec!["immediate".to_string(), "immediate".to_string()],
            vec!["urgent".to_string()],
            vec![
                "immediate".to_string(),
                "standard".to_string(),
                "economy".to_string(),
                "immediate".to_string(),
            ],
        ] {
            let bdk = Bdk {
                batch_config: BatchConfig {
                    fee_options,
                    ..BatchConfig::default()
                },
                ..Default::default()
            };

            let err = bdk.validate().expect_err("invalid fee options should fail");

            assert!(err.contains("fee_options"));
        }
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_target_block_time_zero_rejected() {
        let bdk = Bdk {
            batch_config: BatchConfig {
                target_block_time_secs: 0,
                ..BatchConfig::default()
            },
            ..Default::default()
        };

        let err = bdk
            .validate()
            .expect_err("zero target block time should fail");

        assert!(err.contains("target_block_time_secs"));
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn test_bdk_min_send_amount_sat_zero_rejected() {
        let bdk = Bdk {
            min_send_amount_sat: 0,
            ..Default::default()
        };

        let err = bdk.validate().expect_err("zero send minimum should fail");

        assert!(err.contains("min_send_amount_sat"));
    }

    #[cfg(all(feature = "fakewallet", feature = "bdk"))]
    #[test]
    fn test_fakewallet_backend_with_bdk_onchain_rejected() {
        let settings = Settings {
            payment_backend: vec![PaymentBackend {
                backend: PaymentBackendType::FakeWallet,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::Bdk,
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = settings
            .validate_backend_pairing()
            .expect_err("fake payment backend with BDK onchain should fail");

        assert!(err.contains("fakewallet"));
        assert!(err.contains("bdk"));
    }

    #[cfg(all(feature = "fakewallet", feature = "cln"))]
    #[test]
    fn test_real_backend_with_fakewallet_onchain_rejected() {
        let settings = Settings {
            payment_backend: vec![PaymentBackend {
                backend: PaymentBackendType::Cln,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            ..Default::default()
        };

        let err = settings
            .validate_backend_pairing()
            .expect_err("real payment backend with fake onchain should fail");

        assert!(err.contains("fakewallet"));
        assert!(err.contains("real payment backend"));
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_fakewallet_backend_with_fakewallet_onchain_accepted() {
        let settings = Settings {
            payment_backend: vec![PaymentBackend {
                backend: PaymentBackendType::FakeWallet,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            ..Default::default()
        };

        settings
            .validate_backend_pairing()
            .expect("fake-only backend pairing should pass");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_fakewallet_backend_with_no_onchain_accepted() {
        let settings = Settings {
            payment_backend: vec![PaymentBackend {
                backend: PaymentBackendType::FakeWallet,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::None,
                ..Default::default()
            }),
            ..Default::default()
        };

        settings
            .validate_backend_pairing()
            .expect("fake payment backend without onchain should pass");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_no_payment_backend_with_fakewallet_onchain_accepted() {
        let settings = Settings {
            payment_backend: vec![PaymentBackend {
                backend: PaymentBackendType::None,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            ..Default::default()
        };

        settings
            .validate_backend_pairing()
            .expect("fake onchain-only backend pairing should pass");
    }

    #[cfg(all(feature = "fakewallet", feature = "cln"))]
    #[test]
    fn test_fakewallet_backend_with_real_backend_rejected() {
        let settings = Settings {
            payment_backend: vec![
                PaymentBackend {
                    backend: PaymentBackendType::FakeWallet,
                    ..Default::default()
                },
                PaymentBackend {
                    backend: PaymentBackendType::Cln,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let err = settings
            .validate_backend_pairing()
            .expect_err("fake payment backend combined with real backend should fail");

        assert!(err.contains("fakewallet"));
        assert!(err.contains("real payment backend"));
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_fakewallet_custom_payment_method_unit_matching() {
        let global = FakeWalletCustomPaymentMethod::Method("paypal".to_string());
        let usd_only = FakeWalletCustomPaymentMethod::MethodForUnit {
            method: "venmo".to_string(),
            unit: CurrencyUnit::Usd,
        };

        assert!(global.applies_to_unit(&CurrencyUnit::Sat));
        assert!(global.applies_to_unit(&CurrencyUnit::Usd));
        assert!(!usd_only.applies_to_unit(&CurrencyUnit::Sat));
        assert!(usd_only.applies_to_unit(&CurrencyUnit::Usd));
    }

    #[test]
    fn test_info_debug_with_special_chars() {
        // Test with a mnemonic containing special characters
        let info = Info {
            url: "http://example.com".to_string(),
            listen_host: "127.0.0.1".to_string(),
            listen_port: 8080,
            mnemonic: Some("特殊字符 !@#$%^&*()".to_string()), // Special characters
            ..Default::default()
        };

        // This should not panic
        let debug_output = format!("{:?}", info);

        // The mnemonic with special chars should be hashed
        assert!(!debug_output.contains("特殊字符 !@#$%^&*()"));
        assert!(debug_output.contains("<hashed: "));
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_multi_backend_config_parses() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_multi_backend_config");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[[payment_backend]]
backend = "fakewallet"
unit = "sat"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000

[[payment_backend]]
backend = "fakewallet"
unit = "eur"
min_mint = 1
max_mint = 1000
min_melt = 1
max_melt = 1000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should load");

        assert_eq!(settings.payment_backend.len(), 2);

        assert_eq!(
            settings.payment_backend[0].backend,
            PaymentBackendType::FakeWallet
        );
        assert_eq!(settings.payment_backend[0].unit, CurrencyUnit::Sat);
        let max_mint_0: u64 = settings.payment_backend[0].max_mint.into();
        assert_eq!(max_mint_0, 500_000);

        assert_eq!(
            settings.payment_backend[1].backend,
            PaymentBackendType::FakeWallet
        );
        assert_eq!(settings.payment_backend[1].unit, CurrencyUnit::Eur);
        let max_mint_1: u64 = settings.payment_backend[1].max_mint.into();
        assert_eq!(max_mint_1, 1_000);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_single_payment_backend_block_parses() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_single_payment_backend_block");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[payment_backend]
backend = "fakewallet"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should load");

        assert_eq!(settings.payment_backend.len(), 1);
        assert_eq!(
            settings.payment_backend[0].backend,
            PaymentBackendType::FakeWallet
        );
        assert_eq!(settings.payment_backend[0].unit, CurrencyUnit::Sat);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_fakewallet_config_without_supported_units_parses() {
        use std::{env, fs};

        let temp_dir = env::temp_dir().join("cdk_test_fakewallet_without_supported_units");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[[payment_backend]]
backend = "fakewallet"
unit = "sat"
min_mint = 100
max_mint = 1000000
min_melt = 100
max_melt = 1000000

[fake_wallet]
fee_percent = 0.02
reserve_fee_min = 1
min_delay_time = 1
max_delay_time = 3
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings = Settings::try_new(Some(&config_path)).expect("config should parse");

        assert_eq!(settings.payment_backend.len(), 1);
        assert_eq!(settings.payment_backend[0].unit, CurrencyUnit::Sat);
        assert_eq!(
            settings
                .fake_wallet
                .expect("fake wallet section should parse")
                .supported_units,
            vec![CurrencyUnit::Sat]
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    /// Test that configuration can be loaded purely from environment variables
    /// without requiring a config.toml file with backend sections.
    ///
    /// This test runs sequentially for all enabled backends to avoid env var interference.
    #[test]
    fn test_env_var_only_config_all_backends() {
        let _guard = config_env_lock();

        // Run each backend test sequentially
        #[cfg(feature = "lnd")]
        test_lnd_env_config();

        #[cfg(feature = "cln")]
        test_cln_env_config();

        #[cfg(feature = "fakewallet")]
        test_fakewallet_env_config();

        #[cfg(feature = "grpc-processor")]
        test_grpc_processor_env_config();

        #[cfg(feature = "ldk-node")]
        test_ldk_node_env_config();
    }

    #[cfg(all(feature = "prometheus", feature = "fakewallet"))]
    #[test]
    fn test_prometheus_toml_config_survives_env_overlay() {
        use std::{env, fs};

        let _guard = config_env_lock();

        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_ENABLED);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_ADDRESS);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_PORT);

        let temp_dir =
            env::temp_dir().join(format!("cdk_prometheus_config_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[info]
url = "http://127.0.0.1:8085"
listen_host = "127.0.0.1"
listen_port = 8085

[payment_backend]
backend = "fakewallet"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000

[prometheus]
enabled = true
address = "0.0.0.0"
port = 9090
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let mut settings = Settings::try_new(Some(&config_path)).expect("config should load");
        settings.from_env().expect("Failed to apply env vars");

        let prometheus = settings
            .prometheus
            .as_ref()
            .expect("Prometheus config should be loaded from TOML");
        assert!(prometheus.enabled);
        assert_eq!(prometheus.address.as_deref(), Some("0.0.0.0"));
        assert_eq!(prometheus.port, Some(9090));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "lnd")]
    fn test_lnd_env_config() {
        use std::path::PathBuf;
        use std::{env, fs};

        // Create a temporary directory for config file
        let temp_dir = env::temp_dir().join("cdk_test_env_vars");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        // Create a minimal config.toml with backend set but NO [lnd] section
        let config_content = r#"
[payment_backend]
backend = "lnd"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        // Set environment variables for LND configuration
        env::set_var(crate::env_vars::ENV_PAYMENT_BACKEND, "lnd");
        env::set_var(crate::env_vars::ENV_LND_ADDRESS, "https://localhost:10009");
        env::set_var(crate::env_vars::ENV_LND_CERT_FILE, "/tmp/test_tls.cert");
        env::set_var(
            crate::env_vars::ENV_LND_MACAROON_FILE,
            "/tmp/test_admin.macaroon",
        );
        env::set_var(crate::env_vars::ENV_LND_FEE_PERCENT, "0.01");
        env::set_var(crate::env_vars::ENV_LND_RESERVE_FEE_MIN, "4");

        // Load settings and apply environment variables (same as production code)
        let mut settings = Settings::try_new(Some(&config_path)).expect("Failed to load config");
        settings.from_env().expect("Failed to apply env vars");

        // Verify that settings were populated from env vars
        assert!(settings.lnd.is_some());
        let lnd_config = settings.lnd.as_ref().unwrap();
        assert_eq!(lnd_config.address, "https://localhost:10009");
        assert_eq!(lnd_config.cert_file, PathBuf::from("/tmp/test_tls.cert"));
        assert_eq!(
            lnd_config.macaroon_file,
            PathBuf::from("/tmp/test_admin.macaroon")
        );
        assert_eq!(lnd_config.fee_percent, 0.01);
        let reserve_fee_u64: u64 = lnd_config.reserve_fee_min.into();
        assert_eq!(reserve_fee_u64, 4);

        // Cleanup env vars
        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::ENV_LND_ADDRESS);
        env::remove_var(crate::env_vars::ENV_LND_CERT_FILE);
        env::remove_var(crate::env_vars::ENV_LND_MACAROON_FILE);
        env::remove_var(crate::env_vars::ENV_LND_FEE_PERCENT);
        env::remove_var(crate::env_vars::ENV_LND_RESERVE_FEE_MIN);

        // Cleanup test file
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "cln")]
    fn test_cln_env_config() {
        use std::path::PathBuf;
        use std::{env, fs};

        // Create a temporary directory for config file
        let temp_dir = env::temp_dir().join("cdk_test_env_vars_cln");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        // Create a minimal config.toml with backend set but NO [cln] section
        let config_content = r#"
[payment_backend]
backend = "cln"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        // Set environment variables for CLN configuration
        env::set_var(crate::env_vars::ENV_PAYMENT_BACKEND, "cln");
        env::set_var(crate::env_vars::ENV_CLN_RPC_PATH, "/tmp/lightning-rpc");
        env::set_var(crate::env_vars::ENV_CLN_BOLT12, "false");
        env::set_var(crate::env_vars::ENV_CLN_FEE_PERCENT, "0.01");
        env::set_var(crate::env_vars::ENV_CLN_RESERVE_FEE_MIN, "4");

        // Load settings and apply environment variables (same as production code)
        let mut settings = Settings::try_new(Some(&config_path)).expect("Failed to load config");
        settings.from_env().expect("Failed to apply env vars");

        // Verify that settings were populated from env vars
        assert!(settings.cln.is_some());
        let cln_config = settings.cln.as_ref().unwrap();
        assert_eq!(cln_config.rpc_path, PathBuf::from("/tmp/lightning-rpc"));
        assert!(!cln_config.bolt12);
        assert_eq!(cln_config.fee_percent, 0.01);
        let reserve_fee_u64: u64 = cln_config.reserve_fee_min.into();
        assert_eq!(reserve_fee_u64, 4);

        // Cleanup env vars
        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::ENV_CLN_RPC_PATH);
        env::remove_var(crate::env_vars::ENV_CLN_BOLT12);
        env::remove_var(crate::env_vars::ENV_CLN_FEE_PERCENT);
        env::remove_var(crate::env_vars::ENV_CLN_RESERVE_FEE_MIN);

        // Cleanup test file
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fakewallet")]
    fn test_fakewallet_env_config() {
        use std::{env, fs};

        // Create a temporary directory for config file
        let temp_dir = env::temp_dir().join("cdk_test_env_vars_fakewallet");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        // Create a minimal config.toml with backend set but NO [fake_wallet] section
        let config_content = r#"
[payment_backend]
backend = "fakewallet"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        // Set environment variables for FakeWallet configuration
        env::set_var(crate::env_vars::ENV_PAYMENT_BACKEND, "fakewallet");
        env::set_var(crate::env_vars::ENV_FAKE_WALLET_SUPPORTED_UNITS, "sat,msat");
        env::set_var(crate::env_vars::ENV_FAKE_WALLET_FEE_PERCENT, "0.0");
        env::set_var(crate::env_vars::ENV_FAKE_WALLET_RESERVE_FEE_MIN, "0");
        env::set_var(
            crate::env_vars::ENV_FAKE_WALLET_CUSTOM_PAYMENT_METHODS,
            "venmo:msat,cashapp:sat,paypal",
        );
        env::set_var(crate::env_vars::ENV_FAKE_WALLET_MIN_DELAY, "0");
        env::set_var(crate::env_vars::ENV_FAKE_WALLET_MAX_DELAY, "5");

        // Load settings and apply environment variables (same as production code)
        let mut settings = Settings::try_new(Some(&config_path)).expect("Failed to load config");
        settings.from_env().expect("Failed to apply env vars");

        // Verify that settings were populated from env vars
        assert!(settings.fake_wallet.is_some());
        let fakewallet_config = settings.fake_wallet.as_ref().unwrap();
        assert_eq!(fakewallet_config.fee_percent, 0.0);
        let reserve_fee_u64: u64 = fakewallet_config.reserve_fee_min.into();
        assert_eq!(reserve_fee_u64, 0);
        assert_eq!(
            fakewallet_config.custom_payment_methods,
            vec![
                FakeWalletCustomPaymentMethod::MethodForUnit {
                    method: "venmo".to_string(),
                    unit: CurrencyUnit::Msat,
                },
                FakeWalletCustomPaymentMethod::MethodForUnit {
                    method: "cashapp".to_string(),
                    unit: CurrencyUnit::Sat,
                },
                FakeWalletCustomPaymentMethod::Method("paypal".to_string()),
            ]
        );
        assert_eq!(fakewallet_config.min_delay_time, 0);
        assert_eq!(fakewallet_config.max_delay_time, 5);
        assert_eq!(
            settings
                .payment_backend
                .iter()
                .map(|backend| backend.unit.clone())
                .collect::<Vec<_>>(),
            vec![CurrencyUnit::Sat, CurrencyUnit::Msat]
        );

        // Cleanup env vars
        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_SUPPORTED_UNITS);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_FEE_PERCENT);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_RESERVE_FEE_MIN);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_CUSTOM_PAYMENT_METHODS);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_MIN_DELAY);
        env::remove_var(crate::env_vars::ENV_FAKE_WALLET_MAX_DELAY);

        // Cleanup test file
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "grpc-processor")]
    fn test_grpc_processor_env_config() {
        use std::{env, fs};

        // Create a temporary directory for config file
        let temp_dir = env::temp_dir().join("cdk_test_env_vars_grpc");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        // Create a minimal config.toml with backend set but NO [grpc_processor] section
        let config_content = r#"
[payment_backend]
backend = "grpcprocessor"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        // Set environment variables for GRPC Processor configuration
        env::set_var(crate::env_vars::ENV_PAYMENT_BACKEND, "grpcprocessor");
        env::set_var(
            crate::env_vars::ENV_GRPC_PROCESSOR_SUPPORTED_UNITS,
            "sat,msat",
        );
        env::set_var(crate::env_vars::ENV_GRPC_PROCESSOR_ADDRESS, "localhost");
        env::set_var(crate::env_vars::ENV_GRPC_PROCESSOR_PORT, "50051");

        // Load settings and apply environment variables (same as production code)
        let mut settings = Settings::try_new(Some(&config_path)).expect("Failed to load config");
        settings.from_env().expect("Failed to apply env vars");

        // Verify that settings were populated from env vars
        assert!(settings.grpc_processor.is_some());
        let grpc_config = settings.grpc_processor.as_ref().unwrap();
        assert_eq!(grpc_config.address, "localhost");
        assert_eq!(grpc_config.port, 50051);

        // Cleanup env vars
        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::ENV_GRPC_PROCESSOR_SUPPORTED_UNITS);
        env::remove_var(crate::env_vars::ENV_GRPC_PROCESSOR_ADDRESS);
        env::remove_var(crate::env_vars::ENV_GRPC_PROCESSOR_PORT);

        // Cleanup test file
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "ldk-node")]
    fn test_ldk_node_env_config() {
        use std::{env, fs};

        // Create a temporary directory for config file
        let temp_dir = env::temp_dir().join("cdk_test_env_vars_ldk");
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        // Create a minimal config.toml with backend set but NO [ldk_node] section
        let config_content = r#"
[payment_backend]
backend = "ldknode"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        // Set environment variables for LDK Node configuration
        env::set_var(crate::env_vars::ENV_PAYMENT_BACKEND, "ldknode");
        env::set_var(crate::env_vars::LDK_NODE_FEE_PERCENT_ENV_VAR, "0.01");
        env::set_var(crate::env_vars::LDK_NODE_RESERVE_FEE_MIN_ENV_VAR, "4");
        env::set_var(crate::env_vars::LDK_NODE_BITCOIN_NETWORK_ENV_VAR, "regtest");
        env::set_var(
            crate::env_vars::LDK_NODE_CHAIN_SOURCE_TYPE_ENV_VAR,
            "esplora",
        );
        env::set_var(
            crate::env_vars::LDK_NODE_ESPLORA_URL_ENV_VAR,
            "http://localhost:3000",
        );
        env::set_var(
            crate::env_vars::LDK_NODE_ELECTRUM_URL_ENV_VAR,
            "tcp://localhost:50001",
        );
        env::set_var(
            crate::env_vars::LDK_NODE_STORAGE_DIR_PATH_ENV_VAR,
            "/tmp/ldk",
        );

        // Load settings and apply environment variables (same as production code)
        let mut settings = Settings::try_new(Some(&config_path)).expect("Failed to load config");
        settings.from_env().expect("Failed to apply env vars");

        // Verify that settings were populated from env vars
        assert!(settings.ldk_node.is_some());
        let ldk_config = settings.ldk_node.as_ref().unwrap();
        assert_eq!(ldk_config.fee_percent, 0.01);
        let reserve_fee_u64: u64 = ldk_config.reserve_fee_min.into();
        assert_eq!(reserve_fee_u64, 4);
        assert_eq!(ldk_config.bitcoin_network, Some("regtest".to_string()));
        assert_eq!(ldk_config.chain_source_type, Some("esplora".to_string()));
        assert_eq!(
            ldk_config.esplora_url,
            Some("http://localhost:3000".to_string())
        );
        assert_eq!(
            ldk_config.electrum_url,
            Some("tcp://localhost:50001".to_string())
        );
        assert_eq!(ldk_config.storage_dir_path, Some("/tmp/ldk".to_string()));

        // Cleanup env vars
        env::remove_var(crate::env_vars::ENV_PAYMENT_BACKEND);
        env::remove_var(crate::env_vars::LDK_NODE_FEE_PERCENT_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_RESERVE_FEE_MIN_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_BITCOIN_NETWORK_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_CHAIN_SOURCE_TYPE_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_ESPLORA_URL_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_ELECTRUM_URL_ENV_VAR);
        env::remove_var(crate::env_vars::LDK_NODE_STORAGE_DIR_PATH_ENV_VAR);

        // Cleanup test file
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
