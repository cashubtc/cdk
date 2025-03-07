use std::path::PathBuf;

use bitcoin::hashes::{sha256, Hash};
use cdk::nuts::{CurrencyUnit, PublicKey};
use cdk::Amount;
use cdk_axum::cache;
use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Info {
    pub url: String,
    pub listen_host: String,
    pub listen_port: u16,
    pub mnemonic: String,
    pub input_fee_ppk: Option<u64>,

    pub http_cache: cache::Config,

    /// When this is set to true, the mint exposes a Swagger UI for it's API at
    /// `[listen_host]:[listen_port]/swagger-ui`
    ///
    /// This requires `mintd` was built with the `swagger` feature flag.
    pub enable_swagger_ui: Option<bool>,
}

impl std::fmt::Debug for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mnemonic_hash = sha256::Hash::from_slice(&self.mnemonic.clone().into_bytes())
            .map_err(|_| std::fmt::Error)?;

        f.debug_struct("Info")
            .field("url", &self.url)
            .field("listen_host", &self.listen_host)
            .field("listen_port", &self.listen_port)
            .field("mnemonic", &format!("<hashed: {}>", mnemonic_hash))
            .field("input_fee_ppk", &self.input_fee_ppk)
            .field("http_cache", &self.http_cache)
            .field("enable_swagger_ui", &self.enable_swagger_ui)
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LnBackend {
    #[default]
    None,
    Cln,
    LNbits,
    FakeWallet,
    Lnd,
}

impl std::str::FromStr for LnBackend {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cln" => Ok(LnBackend::Cln),
            "lnbits" => Ok(LnBackend::LNbits),
            "fakewallet" => Ok(LnBackend::FakeWallet),
            "lnd" => Ok(LnBackend::Lnd),
            _ => Err(format!("Unknown Lightning backend: {}", s)),
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LNbits {
    pub admin_api_key: String,
    pub invoice_api_key: String,
    pub lnbits_api: String,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cln {
    pub rpc_path: PathBuf,
    #[serde(default)]
    pub bolt12: bool,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lnd {
    pub address: String,
    pub cert_file: PathBuf,
    pub macaroon_file: PathBuf,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

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
fn default_min_delay_time() -> u64 {
    1
}

fn default_max_delay_time() -> u64 {
    3
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseEngine {
    #[default]
    Sqlite,
    #[cfg(feature = "redb")]
    Redb,
}

impl std::str::FromStr for DatabaseEngine {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sqlite" => Ok(DatabaseEngine::Sqlite),
            #[cfg(feature = "redb")]
            "redb" => Ok(DatabaseEngine::Redb),
            _ => Err(format!("Unknown database engine: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Database {
    pub engine: DatabaseEngine,
}

/// CDK settings, derived from `config.toml`
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub info: Info,
    pub mint_info: MintInfo,
    pub ln: Ln,
    pub cln: Option<Cln>,
    pub lnbits: Option<LNbits>,
    pub lnd: Option<Lnd>,
    pub fake_wallet: Option<FakeWallet>,
    pub database: Database,
    #[cfg(feature = "management-rpc")]
    pub mint_management_rpc: Option<MintManagementRpc>,
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
            LnBackend::Cln => assert!(
                settings.cln.is_some(),
                "CLN backend requires a valid config."
            ),
            LnBackend::LNbits => assert!(
                settings.lnbits.is_some(),
                "LNbits backend requires a valid config"
            ),
            LnBackend::Lnd => {
                assert!(
                    settings.lnd.is_some(),
                    "LND backend requires a valid config."
                )
            }
            LnBackend::FakeWallet => assert!(
                settings.fake_wallet.is_some(),
                "FakeWallet backend requires a valid config."
            ),
        }

        Ok(settings)
    }
}
