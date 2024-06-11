//! CDK Mint Server

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::anyhow;
use cdk::cdk_database::{self, MintDatabase};
use cdk::cdk_lightning::MintLightning;
use cdk::mint::Mint;
use cdk::nuts::MintInfo;
use cdk::{cdk_lightning, Amount, Mnemonic};
use cdk_cln::Cln;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use cli::CLIArgs;
use config::{DatabaseEngine, LnBackend};

mod cli;
mod config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let args = CLIArgs::parse();

    // get config file name from args
    let config_file_arg = match args.config {
        Some(c) => c,
        None => "./config.toml".to_string(),
    };

    let settings = config::Settings::new(&Some(config_file_arg));

    let db_path = match args.db {
        Some(path) => PathBuf::from_str(&path)?,
        None => settings.info.clone().db_path,
    };

    let localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync> =
        match settings.database.engine {
            DatabaseEngine::Sqlite => {
                let sqlite_db = MintSqliteDatabase::new(db_path.to_str().unwrap()).await?;

                sqlite_db.migrate().await;

                Arc::new(sqlite_db)
            }
            DatabaseEngine::Redb => Arc::new(MintRedbDatabase::new(db_path.to_str().unwrap())?),
        };

    let mint_info = MintInfo::default();

    let mnemonic = Mnemonic::from_str(&settings.info.mnemonic)?;

    let mint = Mint::new(
        &mnemonic.to_seed_normalized(""),
        mint_info,
        localstore,
        Amount::ZERO,
        0.0,
    )
    .await?;

    let ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync> =
        match settings.ln.ln_backend {
            LnBackend::Cln => {
                let cln_socket = expand_path(
                    settings
                        .ln
                        .cln_path
                        .clone()
                        .ok_or(anyhow!("cln socket not defined"))?
                        .to_str()
                        .ok_or(anyhow!("cln socket not defined"))?,
                )
                .ok_or(anyhow!("cln socket not defined"))?;

                Arc::new(Cln::new(cln_socket, None).await?)
            }
        };

    let mint_url = settings.info.url;
    let listen_addr = settings.info.listen_host;
    let listen_port = settings.info.listen_port;

    cdk_axum::start_server(&mint_url, &listen_addr, listen_port, mint, ln).await?;

    Ok(())
}

fn expand_path(path: &str) -> Option<PathBuf> {
    if path.starts_with('~') {
        if let Some(home_dir) = dirs::home_dir().as_mut() {
            let remainder = &path[2..];
            home_dir.push(remainder);
            let expanded_path = home_dir;
            Some(expanded_path.clone())
        } else {
            None
        }
    } else {
        Some(PathBuf::from(path))
    }
}
