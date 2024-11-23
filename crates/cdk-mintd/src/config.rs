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

    /// When this is set to true, the mint exposes a Swagger UI for it's API at
    /// `[listen_host]:[listen_port]/swagger-ui`
    ///
    /// This requires `mintd` was built with the `swagger` feature flag.
    pub enable_swagger_ui: Option<bool>,
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
pub struct Strike {
    pub api_key: String,
    pub supported_units: Option<Vec<CurrencyUnit>>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Phoenixd {
    pub api_password: String,
    pub api_url: String,
    pub bolt12: bool,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeWallet {
    pub supported_units: Vec<CurrencyUnit>,
    pub fee_percent: f32,
    pub reserve_fee_min: Amount,
    pub min_delay_time: u64,
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

/// CDK settings, derived from `config.toml`
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
            LnBackend::Cln => assert!(
                settings.cln.is_some(),
                "CLN backend requires a valid config."
            ),
            LnBackend::Strike => assert!(
                settings.strike.is_some(),
                "Strike backend requires a valid config."
            ),
            LnBackend::LNbits => assert!(
                settings.lnbits.is_some(),
                "LNbits backend requires a valid config"
            ),
            LnBackend::Phoenixd => assert!(
                settings.phoenixd.is_some(),
                "Phoenixd backend requires a valid config"
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
