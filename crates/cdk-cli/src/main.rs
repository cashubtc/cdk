use std::fs;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use cdk::cdk_database::WalletDatabase;
use cdk::wallet::Wallet;
use cdk::{cdk_database, Mnemonic};
use cdk_redb::RedbWalletDatabase;
use cdk_sqlite::WalletSQLiteDatabase;
use clap::{Parser, Subcommand};
use rand::Rng;

mod sub_commands;

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
    #[arg(short, long, default_value = "./seed")]
    seed_path: String,
    /// File Path to save proofs
    #[arg(short, long)]
    db_path: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

const DEFAULT_REDB_DB_PATH: &str = "./cashu_tool.redb";
const DEFAULT_SQLITE_DB_PATH: &str = "./cashu_tool.sqlite";

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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // Parse input
    let args: Cli = Cli::parse();

    let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> =
        match args.engine.as_str() {
            "sqlite" => Arc::new(RedbWalletDatabase::new(DEFAULT_REDB_DB_PATH)?),
            "redb" => Arc::new(WalletSQLiteDatabase::new(DEFAULT_SQLITE_DB_PATH).await?),
            _ => bail!("Unknown DB engine"),
        };

    let mnemonic = match fs::metadata(args.seed_path.clone()) {
        Ok(_) => {
            let contents = fs::read_to_string(args.seed_path.clone())?;
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
    }
}
