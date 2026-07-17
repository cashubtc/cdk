//! Verify an Enclavia-hosted mint and display its NUT-06 mint information.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use enclavia::{Client, Pcrs};
use serde::Deserialize;
use thiserror::Error;

const MINT_INFO_PATH: &str = "/v1/info";

#[derive(Debug, Parser)]
#[command(
    name = "cdk-enclavia-cli",
    version,
    about = "Verify an Enclavia-hosted CDK mint and display its mint information"
)]
struct Cli {
    /// Enclavia WebSocket endpoint for the mint
    #[arg(long, required_unless_present = "config", conflicts_with = "config")]
    endpoint: Option<String>,
    /// Expected PCR0 measurement as hexadecimal
    #[arg(long, required_unless_present = "config", conflicts_with = "config")]
    pcr0: Option<String>,
    /// Expected PCR1 measurement as hexadecimal
    #[arg(long, required_unless_present = "config", conflicts_with = "config")]
    pcr1: Option<String>,
    /// Expected PCR2 measurement as hexadecimal
    #[arg(long, required_unless_present = "config", conflicts_with = "config")]
    pcr2: Option<String>,
    /// Accept debug/QEMU attestation without validating the AWS Nitro certificate chain
    #[arg(long, conflicts_with = "config")]
    debug_mode: bool,
    /// Read the endpoint, PCR values, and debug mode from an Enclavia JSON config
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["endpoint", "pcr0", "pcr1", "pcr2", "debug_mode"]
    )]
    config: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    endpoint: String,
    pcrs: ConfigPcrs,
    #[serde(default)]
    debug_mode: bool,
}

#[derive(Debug, Deserialize)]
struct ConfigPcrs {
    pcr0: String,
    pcr1: String,
    pcr2: String,
}

#[derive(Debug)]
struct ConnectionArgs {
    endpoint: String,
    pcr0: String,
    pcr1: String,
    pcr2: String,
    debug_mode: bool,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("could not read config file {path}: {source}")]
    ReadConfig {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("config file {path} contains invalid JSON: {source}")]
    InvalidConfig {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("endpoint and PCR arguments are incomplete")]
    IncompleteArguments,
    #[error("invalid PCR value: {0}")]
    InvalidPcr(String),
    #[error("could not establish attested Enclavia connection: {0}")]
    Enclavia(#[source] Box<enclavia::Error>),
    #[error(
        "could not validate the production attestation certificate: {source}\nHint: if this is intentionally a debug/QEMU enclave, retry with --debug-mode; never use debug mode for a production enclave"
    )]
    DebugAttestation {
        #[source]
        source: Box<enclavia::Error>,
    },
    #[error("mint returned HTTP status {status} from {MINT_INFO_PATH}: {body}")]
    MintHttpStatus { status: u16, body: String },
    #[error("mint returned invalid JSON from {MINT_INFO_PATH}: {0}")]
    InvalidMintInfo(#[source] serde_json::Error),
    #[error("could not format mint information: {0}")]
    FormatMintInfo(#[source] serde_json::Error),
}

impl From<enclavia::Error> for CliError {
    fn from(error: enclavia::Error) -> Self {
        match &error {
            enclavia::Error::Attestation(message)
                if message
                    .contains("Attempts to parse certificate from PEM and DER encoding failed") =>
            {
                Self::DebugAttestation {
                    source: Box::new(error),
                }
            }
            _ => Self::Enclavia(Box::new(error)),
        }
    }
}

struct Hex<'a>(&'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), CliError> {
    let args = connection_args(Cli::parse())?;
    let pcrs = Pcrs::from_hex(&args.pcr0, &args.pcr1, &args.pcr2)
        .map_err(|error| CliError::InvalidPcr(error.to_string()))?;

    print_pcrs("Expected", &pcrs);

    if args.debug_mode {
        eprintln!(
            "WARNING: debug mode skips AWS Nitro certificate-chain and attestation-signature verification!!!"
        );
    }

    let client = Client::builder(&args.endpoint)
        .pcrs(pcrs)
        .debug_mode(args.debug_mode)
        .build()
        .await?;

    eprintln!("Pinned PCRs verified.");

    let response = client.get(MINT_INFO_PATH).send().await?;

    if !(200..300).contains(&response.status()) {
        return Err(CliError::MintHttpStatus {
            status: response.status(),
            body: String::from_utf8_lossy(response.bytes()).into_owned(),
        });
    }

    let mint_info: serde_json::Value =
        serde_json::from_slice(response.bytes()).map_err(CliError::InvalidMintInfo)?;
    let output = serde_json::to_string_pretty(&mint_info).map_err(CliError::FormatMintInfo)?;
    println!("{output}");

    Ok(())
}

fn connection_args(cli: Cli) -> Result<ConnectionArgs, CliError> {
    match cli.config {
        Some(path) => read_config(&path),
        None => Ok(ConnectionArgs {
            endpoint: cli.endpoint.ok_or(CliError::IncompleteArguments)?,
            pcr0: cli.pcr0.ok_or(CliError::IncompleteArguments)?,
            pcr1: cli.pcr1.ok_or(CliError::IncompleteArguments)?,
            pcr2: cli.pcr2.ok_or(CliError::IncompleteArguments)?,
            debug_mode: cli.debug_mode,
        }),
    }
}

fn read_config(path: &Path) -> Result<ConnectionArgs, CliError> {
    let display_path = path.display().to_string();
    let contents = fs::read(path).map_err(|source| CliError::ReadConfig {
        path: display_path.clone(),
        source,
    })?;
    let config: ConfigFile =
        serde_json::from_slice(&contents).map_err(|source| CliError::InvalidConfig {
            path: display_path,
            source,
        })?;

    Ok(ConnectionArgs {
        endpoint: config.endpoint,
        pcr0: config.pcrs.pcr0,
        pcr1: config.pcrs.pcr1,
        pcr2: config.pcrs.pcr2,
        debug_mode: config.debug_mode,
    })
}

fn print_pcrs(label: &str, pcrs: &Pcrs) {
    eprintln!("{label} PCRs:");
    eprintln!("  PCR0: {}", Hex(&pcrs.pcr0));
    eprintln!("  PCR1: {}", Hex(&pcrs.pcr1));
    eprintln!("  PCR2: {}", Hex(&pcrs.pcr2));
}
