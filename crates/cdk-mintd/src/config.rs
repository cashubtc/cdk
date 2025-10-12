use std::path::PathBuf;

use bitcoin::hashes::{sha256, Hash};
use cdk::nuts::{CurrencyUnit, PublicKey};
use cdk::Amount;
use cdk_axum::cache;
use cdk_common::common::QuoteTTL;
use config::{Config, ConfigError, File};
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
pub struct Info {
    pub url: String,
    pub listen_host: String,
    pub listen_port: u16,
    /// Overrides mnemonic
    pub seed: Option<String>,
    pub mnemonic: Option<String>,
    pub signatory_url: Option<String>,
    pub signatory_certs: Option<String>,
    pub input_fee_ppk: Option<u64>,

    pub http_cache: cache::Config,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// When this is set to true, the mint exposes a Swagger UI for it's API at
    /// `[listen_host]:[listen_port]/swagger-ui`
    ///
    /// This requires `mintd` was built with the `swagger` feature flag.
    pub enable_swagger_ui: Option<bool>,

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
            signatory_url: None,
            signatory_certs: None,
            input_fee_ppk: None,
            http_cache: cache::Config::default(),
            enable_swagger_ui: None,
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
                format!("<url: {}>", self.signatory_url.clone().unwrap_or_default())
            }
        };

        f.debug_struct("Info")
            .field("url", &self.url)
            .field("listen_host", &self.listen_host)
            .field("listen_port", &self.listen_port)
            .field("mnemonic", &mnemonic_display)
            .field("input_fee_ppk", &self.input_fee_ppk)
            .field("http_cache", &self.http_cache)
            .field("logging", &self.logging)
            .field("enable_swagger_ui", &self.enable_swagger_ui)
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LnBackend {
    #[default]
    None,
    #[cfg(feature = "cln")]
    Cln,
    #[cfg(feature = "lnbits")]
    LNbits,
    #[cfg(feature = "fakewallet")]
    FakeWallet,
    #[cfg(feature = "lnd")]
    Lnd,
    #[cfg(feature = "ldk-node")]
    LdkNode,
    #[cfg(feature = "grpc-processor")]
    GrpcProcessor,
}

impl std::str::FromStr for LnBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            #[cfg(feature = "cln")]
            "cln" => Ok(LnBackend::Cln),
            #[cfg(feature = "lnbits")]
            "lnbits" => Ok(LnBackend::LNbits),
            #[cfg(feature = "fakewallet")]
            "fakewallet" => Ok(LnBackend::FakeWallet),
            #[cfg(feature = "lnd")]
            "lnd" => Ok(LnBackend::Lnd),
            #[cfg(feature = "ldk-node")]
            "ldk-node" | "ldknode" => Ok(LnBackend::LdkNode),
            #[cfg(feature = "grpc-processor")]
            "grpcprocessor" => Ok(LnBackend::GrpcProcessor),
            _ => Err(format!("Unknown Lightning backend: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ln {
    pub ln_backend: LnBackend,
    pub invoice_description: Option<String>,
    pub min_mint: Amount,
    pub max_mint: Amount,
    pub min_melt: Amount,
    pub max_melt: Amount,
}

impl Default for Ln {
    fn default() -> Self {
        Ln {
            ln_backend: LnBackend::default(),
            invoice_description: None,
            min_mint: 1.into(),
            max_mint: 500_000.into(),
            min_melt: 1.into(),
            max_melt: 500_000.into(),
        }
    }
}

#[cfg(feature = "lnbits")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LNbits {
    pub admin_api_key: String,
    pub invoice_api_key: String,
    pub lnbits_api: String,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[cfg(feature = "cln")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cln {
    pub rpc_path: PathBuf,
    #[serde(default)]
    pub bolt12: bool,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[cfg(feature = "lnd")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lnd {
    pub address: String,
    pub cert_file: PathBuf,
    pub macaroon_file: PathBuf,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[cfg(feature = "ldk-node")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdkNode {
    /// Fee percentage (e.g., 0.02 for 2%)
    #[serde(default = "default_ldk_fee_percent")]
    pub fee_percent: f32,
    /// Minimum reserve fee
    #[serde(default = "default_ldk_reserve_fee_min")]
    pub reserve_fee_min: Amount,
    /// Bitcoin network (mainnet, testnet, signet, regtest)
    pub bitcoin_network: Option<String>,
    /// Chain source type (esplora or bitcoinrpc)
    pub chain_source_type: Option<String>,
    /// Esplora URL (when chain_source_type = "esplora")
    pub esplora_url: Option<String>,
    /// Bitcoin RPC configuration (when chain_source_type = "bitcoinrpc")
    pub bitcoind_rpc_host: Option<String>,
    pub bitcoind_rpc_port: Option<u16>,
    pub bitcoind_rpc_user: Option<String>,
    pub bitcoind_rpc_password: Option<String>,
    /// Storage directory path
    pub storage_dir_path: Option<String>,
    /// LDK node listening host
    pub ldk_node_host: Option<String>,
    /// LDK node listening port
    pub ldk_node_port: Option<u16>,
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
            bitcoind_rpc_host: None,
            bitcoind_rpc_port: None,
            bitcoind_rpc_user: None,
            bitcoind_rpc_password: None,
            storage_dir_path: None,
            ldk_node_host: None,
            ldk_node_port: None,
            gossip_source_type: None,
            rgs_url: None,
            webserver_host: default_webserver_host(),
            webserver_port: default_webserver_port(),
        }
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
pub struct FakeWallet {
    pub supported_units: Vec<CurrencyUnit>,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
    #[serde(default = "default_min_delay_time")]
    pub min_delay_time: u64,
    #[serde(default = "default_max_delay_time")]
    pub max_delay_time: u64,
}

#[cfg(feature = "fakewallet")]
impl Default for FakeWallet {
    fn default() -> Self {
        Self {
            supported_units: vec![CurrencyUnit::Sat],
            fee_percent: 0.02,
            reserve_fee_min: 2.into(),
            min_delay_time: 1,
            max_delay_time: 3,
        }
    }
}

// Helper functions to provide default values
#[cfg(feature = "fakewallet")]
fn default_min_delay_time() -> u64 {
    1
}

#[cfg(feature = "fakewallet")]
fn default_max_delay_time() -> u64 {
    3
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct GrpcProcessor {
    pub supported_units: Vec<CurrencyUnit>,
    pub addr: String,
    pub port: u16,
    pub tls_dir: Option<PathBuf>,
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
pub struct Database {
    pub engine: DatabaseEngine,
    pub postgres: Option<PostgresConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthDatabase {
    pub postgres: Option<PostgresAuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresAuthConfig {
    pub url: String,
    pub tls_mode: Option<String>,
    pub max_connections: Option<usize>,
    pub connection_timeout_seconds: Option<u64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub url: String,
    pub tls_mode: Option<String>,
    pub max_connections: Option<usize>,
    pub connection_timeout_seconds: Option<u64>,
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
pub struct Settings {
    pub info: Info,
    pub mint_info: MintInfo,
    pub ln: Ln,
    #[cfg(feature = "cln")]
    pub cln: Option<Cln>,
    #[cfg(feature = "lnbits")]
    pub lnbits: Option<LNbits>,
    #[cfg(feature = "lnd")]
    pub lnd: Option<Lnd>,
    #[cfg(feature = "ldk-node")]
    pub ldk_node: Option<LdkNode>,
    #[cfg(feature = "fakewallet")]
    pub fake_wallet: Option<FakeWallet>,
    pub grpc_processor: Option<GrpcProcessor>,
    pub database: Database,
    #[cfg(feature = "auth")]
    pub auth_database: Option<AuthDatabase>,
    #[cfg(feature = "management-rpc")]
    pub mint_management_rpc: Option<MintManagementRpc>,
    pub auth: Option<Auth>,
    #[cfg(feature = "prometheus")]
    pub prometheus: Option<Prometheus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg(feature = "prometheus")]
pub struct Prometheus {
    pub enabled: bool,
    pub address: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
pub struct MintManagementRpc {
    /// When this is set to `true` the mint use the config file for the initial set up on first start.
    /// Changes to the `[mint_info]` after this **MUST** be made via the RPC changes to the config file or env vars will be ignored.
    pub enabled: bool,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub tls_dir_path: Option<PathBuf>,
}

impl Settings {
    #[must_use]
    pub fn new<P>(config_file_name: Option<P>) -> Self
    where
        P: Into<PathBuf>,
    {
        let default_settings = Self::default();
        // attempt to construct settings with file
        let from_file = Self::new_from_default(&default_settings, config_file_name);
        match from_file {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(
                    "Error reading config file, falling back to defaults. Error: {e:?}"
                );
                default_settings
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
        let settings: Settings = config.try_deserialize()?;

        match settings.ln.ln_backend {
            LnBackend::None => panic!("Ln backend must be set"),
            #[cfg(feature = "cln")]
            LnBackend::Cln => assert!(
                settings.cln.is_some(),
                "CLN backend requires a valid config."
            ),
            #[cfg(feature = "lnbits")]
            LnBackend::LNbits => assert!(
                settings.lnbits.is_some(),
                "LNbits backend requires a valid config"
            ),
            #[cfg(feature = "lnd")]
            LnBackend::Lnd => {
                assert!(
                    settings.lnd.is_some(),
                    "LND backend requires a valid config."
                )
            }
            #[cfg(feature = "ldk-node")]
            LnBackend::LdkNode => {
                assert!(
                    settings.ldk_node.is_some(),
                    "LDK Node backend requires a valid config."
                )
            }
            #[cfg(feature = "fakewallet")]
            LnBackend::FakeWallet => assert!(
                settings.fake_wallet.is_some(),
                "FakeWallet backend requires a valid config."
            ),
            #[cfg(feature = "grpc-processor")]
            LnBackend::GrpcProcessor => {
                assert!(
                    settings.grpc_processor.is_some(),
                    "GRPC backend requires a valid config."
                )
            }
        }

        Ok(settings)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

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
            enable_swagger_ui: Some(false),
            ..Default::default()
        };

        // This should not panic
        let debug_output = format!("{:?}", info);

        // The empty mnemonic should still be hashed
        assert!(debug_output.contains("<hashed: "));
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
}
