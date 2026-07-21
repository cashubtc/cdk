//! Mint RPC CLI

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use cdk_mint_rpc::mint_rpc_cli::{self, ManagementCommand};
use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Common CLI arguments for CDK binaries
#[derive(Parser, Debug)]
pub struct CommonArgs {
    /// Enable logging (default is false)
    #[arg(long, default_value_t = false)]
    pub enable_logging: bool,

    /// Logging level when enabled (default is debug)
    #[arg(long, default_value = "debug")]
    pub log_level: tracing::Level,
}

/// Initialize logging based on CLI arguments
pub fn init_logging(enable_logging: bool, log_level: tracing::Level) {
    if enable_logging {
        let default_filter = log_level.to_string();

        // Common filters to reduce noise
        let sqlx_filter = "sqlx=warn";
        let hyper_filter = "hyper=warn";
        let h2_filter = "h2=warn";
        let rustls_filter = "rustls=warn";
        let reqwest_filter = "reqwest=warn";

        let env_filter = EnvFilter::new(format!(
            "{default_filter},{sqlx_filter},{hyper_filter},{h2_filter},{rustls_filter},{reqwest_filter}"
        ));

        // Ok if successful, Err if already initialized
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_ansi(false)
            .try_init();
    }
}

const DEFAULT_WORK_DIR: &str = ".cdk-mint-rpc-cli";

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    /// Address of RPC server
    #[arg(short, long, default_value = "https://127.0.0.1:8086")]
    addr: String,

    /// Path to working dir
    #[arg(short, long)]
    work_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: ManagementCommand,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    // Initialize logging based on CLI arguments
    init_logging(args.common.enable_logging, args.common.log_level);

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir = home::home_dir().ok_or_else(|| anyhow!("Could not find home dir"))?;
            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    std::fs::create_dir_all(&work_dir)?;
    tracing::debug!("Using work dir: {}", work_dir.display());

    // Match main: TLS client credentials live under `<work-dir>/tls` when present.
    let tls_dir = {
        let tls_dir = work_dir.join("tls");
        tls_dir.is_dir().then_some(tls_dir)
    };

    let mut client = cdk_mint_rpc::connect_client(&args.addr, tls_dir.as_deref()).await?;
    mint_rpc_cli::dispatch(&mut client, &args.command).await
}
