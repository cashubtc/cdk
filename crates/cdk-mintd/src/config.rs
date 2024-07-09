use std::path::PathBuf;

use cdk::nuts::PublicKey;
use cdk::Amount;
use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Info {
    pub url: String,
    pub listen_host: String,
    pub listen_port: u16,
    pub mnemonic: String,
    pub seconds_quote_is_valid_for: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LnBackend {
    #[default]
    Cln,
    //  Greenlight,
    //  Ldk,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Ln {
    pub ln_backend: LnBackend,
    pub cln_path: Option<PathBuf>,
    pub greenlight_invite_code: Option<String>,
    pub invoice_description: Option<String>,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
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
                warn!("Error reading config file ({:?})", e);
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

        debug!("{settings:?}");

        match settings.ln.ln_backend {
            LnBackend::Cln => assert!(settings.ln.cln_path.is_some()),
            //LnBackend::Greenlight => (),
            //LnBackend::Ldk => (),
        }

        Ok(settings)
    }
}
