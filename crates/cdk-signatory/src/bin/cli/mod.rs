//! Signatory CLI main logic
//!
//! This logic is in this file to be excluded for wasm
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::{env, fs};

use anyhow::{bail, Result};
use bip39::rand::{thread_rng, Rng};
use bip39::Mnemonic;
use cdk_common::database::MintKeysDatabase;
use cdk_common::CurrencyUnit;
#[cfg(feature = "redb")]
use cdk_redb::MintRedbDatabase;
use cdk_signatory::{db_signatory, start_grpc_server};
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;

const DEFAULT_WORK_DIR: &str = ".cdk-signatory";
const ENV_MNEMONIC: &str = "CDK_MINTD_MNEMONIC";

/// Simple CLI application to interact with cashu
#[derive(Parser)]
#[command(name = "cashu-signatory")]
#[command(author = "thesimplekid <tsk@thesimplekid.com>")]
#[command(version = "0.1.0")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Database engine to use (sqlite/redb)
    #[arg(short, long, default_value = "sqlite")]
    engine: String,
    /// Database password for sqlcipher
    #[arg(long)]
    password: Option<String>,
    /// Path to working dir
    #[arg(short, long)]
    work_dir: Option<PathBuf>,
    /// Logging level
    #[arg(short, long, default_value = "error")]
    log_level: Level,
    #[arg(long, default_value = "127.0.0.1")]
    listen_addr: String,
    #[arg(long, default_value = "15060")]
    listen_port: u32,
    #[arg(long, short)]
    certs: Option<String>,
    /// Supported units with the format of name,fee and max_order
    #[arg(long, short, default_value = "sat,0,32")]
    units: Vec<String>,
}

/// Main function for the signatory standalone binary
pub async fn cli_main() -> Result<()> {
    let args: Cli = Cli::parse();
    let default_filter = args.log_level;
    let supported_units = args
        .units
        .into_iter()
        .map(|unit| {
            let mut parts = unit.split(",").collect::<Vec<_>>();
            parts.reverse();
            let unit: CurrencyUnit = parts.pop().unwrap_or_default().parse()?;
            let fee = parts
                .pop()
                .map(|x| x.parse())
                .transpose()?
                .unwrap_or_default();
            let max_order = parts.pop().map(|x| x.parse()).transpose()?.unwrap_or(32);
            Ok::<(_, (_, _)), anyhow::Error>((unit, (fee, max_order)))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn";

    let env_filter = EnvFilter::new(format!("{default_filter},{sqlx_filter}"));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir = home::home_dir().unwrap();
            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    let certs = Some(
        args.certs
            .map(|x| x.into())
            .unwrap_or_else(|| work_dir.clone()),
    );

    fs::create_dir_all(&work_dir)?;

    let localstore: Arc<dyn MintKeysDatabase<Err = cdk_common::database::Error> + Send + Sync> =
        match args.engine.as_str() {
            "sqlite" => {
                let sql_path = work_dir.join("cdk-cli.sqlite");
                #[cfg(not(feature = "sqlcipher"))]
                let db = MintSqliteDatabase::new(&sql_path).await?;
                #[cfg(feature = "sqlcipher")]
                let db = {
                    match args.password {
                        Some(pass) => MintSqliteDatabase::new(&sql_path, pass).await?,
                        None => bail!("Missing database password"),
                    }
                };

                Arc::new(db)
            }
            "redb" => {
                #[cfg(feature = "redb")]
                {
                    let redb_path = work_dir.join("cdk-cli.redb");
                    Arc::new(MintRedbDatabase::new(&redb_path)?)
                }
                #[cfg(not(feature = "redb"))]
                {
                    bail!("redb feature not enabled");
                }
            }
            _ => bail!("Unknown DB engine"),
        };

    let seed_path = work_dir.join("seed");

    let mnemonic = if let Ok(mnemonic) = env::var(ENV_MNEMONIC) {
        Mnemonic::from_str(&mnemonic)?
    } else {
        match fs::metadata(seed_path.clone()) {
            Ok(_) => {
                let contents = fs::read_to_string(seed_path.clone())?;
                Mnemonic::from_str(&contents)?
            }
            Err(_e) => {
                let mut rng = thread_rng();
                let random_bytes: [u8; 32] = rng.gen();

                let mnemonic = Mnemonic::from_entropy(&random_bytes)?;
                tracing::info!("Creating new seed");

                fs::write(seed_path, mnemonic.to_string())?;

                mnemonic
            }
        }
    };
    let seed = mnemonic.to_seed_normalized("");

    let signatory =
        db_signatory::DbSignatory::new(localstore, &seed, supported_units, Default::default())
            .await?;

    let socket_addr = SocketAddr::from_str(&format!("{}:{}", args.listen_addr, args.listen_port))?;

    start_grpc_server(signatory, socket_addr, certs).await?;

    Ok(())
}
