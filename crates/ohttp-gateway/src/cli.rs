use std::path::PathBuf;

use clap::{value_parser, Parser};
use url::Url;

#[derive(Debug, Parser)]
#[command(
    version = env!("CARGO_PKG_VERSION"),
    about = "OHTTP Gateway",
    long_about = "A generic OHTTP gateway that forwards encapsulated requests to a configured backend without storing data",
)]
pub struct Cli {
    /// The port to bind the gateway on
    #[arg(long, short = 'p', env = "OHTTP_GATEWAY_PORT", default_value = "8080")]
    pub port: u16,

    /// The backend URL to forward requests to
    #[arg(
        long,
        env = "OHTTP_GATEWAY_BACKEND_URL",
        help = "The backend URL to forward requests to",
        default_value = "http://localhost:8080",
        value_parser = validate_url
    )]
    pub backend_url: Url,

    /// The working directory where OHTTP keys will be stored
    #[arg(
        long = "work-dir",
        env = "OHTTP_GATEWAY_WORK_DIR",
        help = "The working directory where OHTTP keys will be stored",
        value_parser = value_parser!(PathBuf)
    )]
    pub work_dir: Option<PathBuf>,
}

impl Cli {
    /// Get the work directory, using default if not specified
    pub fn get_work_dir(&self) -> anyhow::Result<PathBuf> {
        match &self.work_dir {
            Some(dir) => Ok(dir.clone()),
            None => {
                let home_dir = home::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("Unable to determine home directory"))?;
                let dir = home_dir.join(".ohttp-gateway");
                std::fs::create_dir_all(&dir)?;
                Ok(dir)
            }
        }
    }
}

/// Validate that the backend URL is well-formed
fn validate_url(s: &str) -> Result<Url, String> {
    let url = Url::parse(s).map_err(|e| format!("Invalid URL '{}': {}", s, e))?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!(
            "URL must use http or https scheme, got: {}",
            url.scheme()
        ));
    }

    if url.host().is_none() {
        return Err("URL must have a host".to_string());
    }

    Ok(url)
}
