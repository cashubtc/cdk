use std::collections::HashMap;
use std::env;
use std::str::FromStr;

use bip39::Mnemonic;
use cdk::nuts::CurrencyUnit;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::env_vars::ENV_WORK_DIR;
use cdk_mintd::{config, work_dir};
use cdk_signatory::proto::server::grpc_server;
use cdk_signatory::MemorySignatory;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = CLIArgs::parse();
    let work_dir = if let Some(work_dir) = args.work_dir {
        tracing::info!("Using work dir from cmd arg");
        work_dir
    } else if let Ok(env_work_dir) = env::var(ENV_WORK_DIR) {
        tracing::info!("Using work dir from env var");
        env_work_dir.into()
    } else {
        work_dir()?
    };

    let config_file_arg = match args.config {
        Some(c) => c,
        None => work_dir.join("config.toml"),
    };

    let settings = if config_file_arg.exists() {
        config::Settings::new(Some(config_file_arg))
    } else {
        tracing::info!("Config file does not exist. Attempting to read env vars");
        config::Settings::default()
    };

    // This check for any settings defined in ENV VARs
    // ENV VARS will take **priority** over those in the config
    let mut settings = settings.from_env()?;
    let mnemonic = Mnemonic::from_str(&settings.info.mnemonic)?;

    let signatory = MemorySignatory::new(
        settings.database.engine.clone().mint(&work_dir).await?,
        &mnemonic.to_seed_normalized(""),
        settings
            .supported_units
            .take()
            .unwrap_or(vec![CurrencyUnit::default()])
            .into_iter()
            .map(|u| (u, (0, 32)))
            .collect::<HashMap<_, _>>(),
        HashMap::new(),
    )
    .await?;

    grpc_server(signatory, "[::1]:50051".parse().unwrap()).await?;

    Ok(())
}
