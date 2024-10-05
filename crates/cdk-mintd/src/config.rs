use std::path::PathBuf;

use cdk::nuts::{CurrencyUnit, PublicKey};
use cdk::Amount;
use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Info {
    pub url: String,
    pub listen_host: String,
    pub listen_port: u16,
    pub mnemonic: String,
    pub seconds_quote_is_valid_for: Option<u64>,
    pub seconds_to_cache_requests_for: Option<u64>,
    pub seconds_to_extend_cache_by: Option<u64>,
    pub input_fee_ppk: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LnBackend {
    #[default]
    Cln,
    Strike,
    LNbits,
    FakeWallet,
    Phoenixd,
    Lnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ln {
    pub ln_backend: LnBackend,
    pub invoice_description: Option<String>,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Strike {
    pub api_key: String,
    pub supported_units: Option<Vec<CurrencyUnit>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LNbits {
    pub admin_api_key: String,
    pub invoice_api_key: String,
    pub lnbits_api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cln {
    pub rpc_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lnd {
    pub address: String,
    pub cert_file: PathBuf,
    pub macaroon_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Phoenixd {
    pub api_password: String,
    pub api_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeWallet {
    pub supported_units: Vec<CurrencyUnit>,
}

impl Default for FakeWallet {
    fn default() -> Self {
        Self {
            supported_units: vec![CurrencyUnit::Sat],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseEngine {
    #[default]
    Sqlite,
    Redb,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Database {
    pub engine: DatabaseEngine,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    pub info: Info,
    pub mint_info: MintInfo,
    pub ln: Ln,
    pub cln: Option<Cln>,
    pub strike: Option<Strike>,
    pub lnbits: Option<LNbits>,
    pub phoenixd: Option<Phoenixd>,
    pub lnd: Option<Lnd>,
    pub fake_wallet: Option<FakeWallet>,
    pub database: Database,
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

impl Settings {
    #[must_use]
    pub fn new(config_file_name: &Option<PathBuf>) -> Self {
        let default_settings = Self::default();
        // attempt to construct settings with file
        let from_file = Self::new_from_default(&default_settings, config_file_name);
        match from_file {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("Error reading config file ({:?})", e);
                default_settings
            }
        }
    }

    fn new_from_default(
        default: &Settings,
        config_file_name: &Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let mut default_config_file_name = home::home_dir()
            .ok_or(ConfigError::NotFound("Config Path".to_string()))?
            .join("cashu-rs-mint");

        default_config_file_name.push("config.toml");
        let config: String = match config_file_name {
            Some(value) => value.clone().to_string_lossy().to_string(),
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
            LnBackend::Cln => assert!(settings.cln.is_some()),
            LnBackend::Strike => assert!(settings.strike.is_some()),
            LnBackend::LNbits => assert!(settings.lnbits.is_some()),
            LnBackend::Phoenixd => assert!(settings.phoenixd.is_some()),
            LnBackend::Lnd => assert!(settings.lnd.is_some()),
            LnBackend::FakeWallet => (),
        }

        Ok(settings)
    }
}
