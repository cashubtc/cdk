use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::cdk_database::WalletDatabase;
use cdk::wallet::Wallet;
use cdk::{cdk_database, UncheckedUrl};
use cdk_redb::WalletRedbDatabase;
use cdk_sqlite::WalletSqliteDatabase;
use clap::{Parser, Subcommand};
use rand::Rng;
use tracing::Level;
use tracing_subscriber::EnvFilter;

mod sub_commands;

const DEFAULT_WORK_DIR: &str = ".cdk-cli";

/// Simple CLI application to interact with cashu
#[derive(Parser)]
#[command(name = "cashu-tool")]
#[command(author = "thesimplekid <tsk@thesimplekid.com>")]
#[command(version = "0.1.0")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Database engine to use (sqlite/redb)
    #[arg(short, long, default_value = "sqlite")]
    engine: String,
    /// Path to working dir
    #[arg(short, long)]
    work_dir: Option<PathBuf>,
    /// Logging level
    #[arg(short, long, default_value = "error")]
    log_level: Level,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decode a token
    DecodeToken(sub_commands::decode_token::DecodeTokenSubCommand),
    /// Balance
    Balance,
    /// Pay bolt11 invoice
    Pay,
    /// Claim pending mint quotes that have been paid
    MintPending,
    /// Receive token
    Receive(sub_commands::receive::ReceiveSubCommand),
    /// Send
    Send(sub_commands::send::SendSubCommand),
    /// Check if wallet balance is spendable
    CheckSpendable,
    /// View mint info
    MintInfo(sub_commands::mint_info::MintInfoSubcommand),
    /// Mint proofs via bolt11
    Mint(sub_commands::mint::MintSubCommand),
    /// Burn Spent tokens
    Burn(sub_commands::burn::BurnSubCommand),
    /// Restore proofs from seed
    Restore(sub_commands::restore::RestoreSubCommand),
    /// Update Mint Url
    UpdateMintUrl(sub_commands::update_mint_url::UpdateMintUrlSubCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Cli = Cli::parse();
    let default_filter = args.log_level;

    let sqlx_filter = "sqlx=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir = home::home_dir().unwrap();
            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    fs::create_dir_all(&work_dir)?;

    let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> =
        match args.engine.as_str() {
            "sqlite" => {
                let sql_path = work_dir.join("cdk-cli.sqlite");
                let sql = WalletSqliteDatabase::new(&sql_path).await?;

                sql.migrate().await;

                Arc::new(sql)
            }
            "redb" => {
                let redb_path = work_dir.join("cdk-cli.redb");

                Arc::new(WalletRedbDatabase::new(&redb_path)?)
            }
            _ => bail!("Unknown DB engine"),
        };

    let seed_path = work_dir.join("seed");

    let mnemonic = match fs::metadata(seed_path.clone()) {
        Ok(_) => {
            let contents = fs::read_to_string(seed_path.clone())?;
            Mnemonic::from_str(&contents)?
        }
        Err(_e) => {
            let mut rng = rand::thread_rng();
            let random_bytes: [u8; 32] = rng.gen();

            let mnemnic = Mnemonic::from_entropy(&random_bytes)?;
            tracing::info!("Using randomly generated seed you will not be able to restore");

            mnemnic
        }
    };

    let mut wallets: HashMap<UncheckedUrl, Wallet> = HashMap::new();

    let mints = localstore.get_mints().await?;

    for (mint, _) in mints {
        let wallet = Wallet::new(
            &mint.to_string(),
            cdk::nuts::CurrencyUnit::Sat,
            localstore.clone(),
            &mnemonic.to_seed_normalized(""),
            None,
        );

        wallets.insert(mint, wallet);
    }

    match &args.command {
        Commands::DecodeToken(sub_command_args) => {
            sub_commands::decode_token::decode_token(sub_command_args)
        }
        Commands::Balance => sub_commands::balance::balance(wallets).await,
        Commands::Pay => sub_commands::melt::pay(wallets).await,
        Commands::Receive(sub_command_args) => {
            sub_commands::receive::receive(
                wallets,
                &mnemonic.to_seed_normalized(""),
                localstore,
                sub_command_args,
            )
            .await
        }
        Commands::Send(sub_command_args) => {
            sub_commands::send::send(wallets, sub_command_args).await
        }
        Commands::CheckSpendable => sub_commands::check_spent::check_spent(wallets).await,
        Commands::MintInfo(sub_command_args) => {
            sub_commands::mint_info::mint_info(sub_command_args).await
        }
        Commands::Mint(sub_command_args) => {
            sub_commands::mint::mint(
                wallets,
                &mnemonic.to_seed_normalized(""),
                localstore,
                sub_command_args,
            )
            .await
        }
        Commands::MintPending => sub_commands::pending_mints::mint_pending(wallets).await,
        Commands::Burn(sub_command_args) => {
            sub_commands::burn::burn(wallets, sub_command_args).await
        }
        Commands::Restore(sub_command_args) => {
            sub_commands::restore::restore(wallets, sub_command_args).await
        }
        Commands::UpdateMintUrl(sub_command_args) => {
            sub_commands::update_mint_url::update_mint_url(wallets, sub_command_args).await
        }
    }
}
