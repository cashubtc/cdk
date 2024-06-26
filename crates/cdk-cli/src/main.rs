use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::Mnemonic;
use cdk::cdk_database;
use cdk::cdk_database::WalletDatabase;
use cdk::wallet::Wallet;
use cdk_redb::RedbWalletDatabase;
use cdk_sqlite::WalletSQLiteDatabase;
use clap::{Parser, Subcommand};
use rand::Rng;

mod sub_commands;

const DEFAULT_WORK_DIR: &str = ".cdk-cli";

/// Simple CLI application to interact with cashu
#[derive(Parser)]
#[command(name = "cashu-tool")]
#[command(author = "thesimplekid <tsk@thesimplekid.com>")]
#[command(version = "0.1")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Database engine to use (sqlite/redb)
    #[arg(short, long, default_value = "sqlite")]
    engine: String,
    /// Path to Seed
    #[arg(short, long)]
    work_dir: Option<PathBuf>,
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
    Melt(sub_commands::melt::MeltSubCommand),
    /// Claim pending mints
    PendingMint,
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
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // Parse input
    let args: Cli = Cli::parse();

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
                let sql = WalletSQLiteDatabase::new(&sql_path).await?;

                sql.migrate().await;

                Arc::new(sql)
            }
            "redb" => {
                let redb_path = work_dir.join("cdk-cli.redb");

                Arc::new(RedbWalletDatabase::new(&redb_path)?)
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

    let wallet = Wallet::new(localstore, &mnemonic.to_seed_normalized(""), vec![]);

    match &args.command {
        Commands::DecodeToken(sub_command_args) => {
            sub_commands::decode_token::decode_token(sub_command_args)
        }
        Commands::Balance => sub_commands::balance::balance(wallet).await,
        Commands::Melt(sub_command_args) => {
            sub_commands::melt::melt(wallet, sub_command_args).await
        }
        Commands::Receive(sub_command_args) => {
            sub_commands::receive::receive(wallet, sub_command_args).await
        }
        Commands::Send(sub_command_args) => {
            sub_commands::send::send(wallet, sub_command_args).await
        }
        Commands::CheckSpendable => sub_commands::check_spent::check_spent(wallet).await,
        Commands::MintInfo(sub_command_args) => {
            sub_commands::mint_info::mint_info(sub_command_args).await
        }
        Commands::Mint(sub_command_args) => {
            sub_commands::mint::mint(wallet, sub_command_args).await
        }
        Commands::PendingMint => sub_commands::pending_mints::pending_mints(wallet).await,
        Commands::Burn(sub_command_args) => {
            sub_commands::burn::burn(wallet, sub_command_args).await
        }
        Commands::Restore(sub_command_args) => {
            sub_commands::restore::restore(wallet, sub_command_args).await
        }
        Commands::UpdateMintUrl(sub_command_args) => {
            sub_commands::update_mint_url::update_mint_url(wallet, sub_command_args).await
        }
    }
}
