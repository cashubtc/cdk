use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Result};

use crate::config::{Iroh, IrohDiscovery};

const ENV_ENABLED: &str = "CDK_MINTD_IROH_ENABLED";
const ENV_SECRET_KEY_FILE: &str = "CDK_MINTD_IROH_SECRET_KEY_FILE";
const ENV_ENDPOINT_TICKET_FILE: &str = "CDK_MINTD_IROH_ENDPOINT_TICKET_FILE";
const ENV_DISCOVERY: &str = "CDK_MINTD_IROH_DISCOVERY";
const ENV_RELAY_URLS: &str = "CDK_MINTD_IROH_RELAY_URLS";
const ENV_BIND_ADDR: &str = "CDK_MINTD_IROH_BIND_ADDR";

const IROH_ENV_VARS: &[&str] = &[
    ENV_ENABLED,
    ENV_SECRET_KEY_FILE,
    ENV_ENDPOINT_TICKET_FILE,
    ENV_DISCOVERY,
    ENV_RELAY_URLS,
    ENV_BIND_ADDR,
];

pub(super) fn iroh_env_configured() -> bool {
    IROH_ENV_VARS.iter().any(|name| env::var_os(name).is_some())
}

impl Iroh {
    pub(super) fn apply_env(mut self) -> Result<Self> {
        if let Some(value) = parse_env(ENV_ENABLED)? {
            self.enabled = value;
        }
        if let Ok(value) = env::var(ENV_SECRET_KEY_FILE) {
            self.secret_key_file = Some(PathBuf::from(value));
        }
        if let Ok(value) = env::var(ENV_ENDPOINT_TICKET_FILE) {
            self.endpoint_ticket_file = Some(PathBuf::from(value));
        }
        if let Ok(value) = env::var(ENV_DISCOVERY) {
            self.discovery = match value.to_ascii_lowercase().as_str() {
                "n0" => IrohDiscovery::N0,
                "static" => IrohDiscovery::Static,
                "custom" => IrohDiscovery::Custom,
                _ => return Err(anyhow!("invalid value in {ENV_DISCOVERY}")),
            };
        }
        if let Ok(value) = env::var(ENV_RELAY_URLS) {
            self.relay_urls = parse_list(&value);
        }
        if let Some(value) = parse_env(ENV_BIND_ADDR)? {
            self.bind_addr = Some(value);
        }
        Ok(self)
    }
}

fn parse_env<T>(name: &str) -> Result<Option<T>>
where
    T: FromStr,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| anyhow!("invalid value in {name}")),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(anyhow!("non-Unicode value in {name}")),
    }
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}
