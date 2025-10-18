use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use bip39::rand::{thread_rng, Rng};
use bip39::Mnemonic;
use cdk::cdk_database;
use cdk::cdk_database::WalletDatabase;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::MultiMintWallet;
#[cfg(feature = "redb")]
use cdk_redb::WalletRedbDatabase;
use cdk_sqlite::WalletSqliteDatabase;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
use clap::ValueEnum;
use clap::{Parser, Subcommand};
use tracing::Level;
use tracing_subscriber::EnvFilter;
use url::Url;

mod nostr_storage;
mod sub_commands;
mod token_storage;
mod utils;

const DEFAULT_WORK_DIR: &str = ".cdk-cli";
const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

/// Simple CLI application to interact with cashu
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
#[derive(Copy, Clone, Debug, ValueEnum)]
enum TorToggle {
    On,
    Off,
}

#[derive(Parser)]
#[command(name = "cdk-cli", author = "thesimplekid <tsk@thesimplekid.com>", version = CARGO_PKG_VERSION.unwrap_or("Unknown"), about, long_about = None)]
struct Cli {
    /// Database engine to use (sqlite/redb)
    #[arg(short, long, default_value = "sqlite")]
    engine: String,
    /// Database password for sqlcipher
    #[cfg(feature = "sqlcipher")]
    #[arg(long)]
    password: Option<String>,
    /// Path to working dir
    #[arg(short, long)]
    work_dir: Option<PathBuf>,
    /// Logging level
    #[arg(short, long, default_value = "error")]
    log_level: Level,
    /// NWS Proxy
    #[arg(short, long)]
    proxy: Option<Url>,
    /// Currency unit to use for the wallet
    #[arg(short, long, default_value = "sat")]
    unit: String,
    /// Use Tor transport (only when built with --features tor). Defaults to 'on' when feature is enabled.
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    #[arg(long = "tor", value_enum, default_value_t = TorToggle::On)]
    transport: TorToggle,
    /// Subcommand to run
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
    /// Claim pending mint quotes that have been paid
    MintPending,
    /// Receive token
    Receive(sub_commands::receive::ReceiveSubCommand),
    /// Send
    Send(sub_commands::send::SendSubCommand),
    /// Transfer tokens between mints
    Transfer(sub_commands::transfer::TransferSubCommand),
    /// Reclaim pending proofs that are no longer pending
    CheckPending,
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
    /// Get proofs from mint.
    ListMintProofs,
    /// Decode a payment request
    DecodeRequest(sub_commands::decode_request::DecodePaymentRequestSubCommand),
    /// Pay a payment request
    PayRequest(sub_commands::pay_request::PayRequestSubCommand),
    /// Create Payment request
    CreateRequest(sub_commands::create_request::CreateRequestSubCommand),
    /// Mint blind auth proofs
    MintBlindAuth(sub_commands::mint_blind_auth::MintBlindAuthSubCommand),
    /// Cat login with username/password
    CatLogin(sub_commands::cat_login::CatLoginSubCommand),
    /// Cat login with device code flow
    CatDeviceLogin(sub_commands::cat_device_login::CatDeviceLoginSubCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Cli = Cli::parse();
    let default_filter = args.log_level;

    let filter = "rustls=warn,hyper_util=warn,reqwest=warn";

    let env_filter = EnvFilter::new(format!("{default_filter},{filter}"));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir = home::home_dir().unwrap();
            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    // Create work directory if it doesn't exist
    if !work_dir.exists() {
        fs::create_dir_all(&work_dir)?;
    }

    let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> =
        match args.engine.as_str() {
            "sqlite" => {
                let sql_path = work_dir.join("cdk-cli.sqlite");
                #[cfg(not(feature = "sqlcipher"))]
                let sql = WalletSqliteDatabase::new(&sql_path).await?;
                #[cfg(feature = "sqlcipher")]
                let sql = {
                    match args.password {
                        Some(pass) => WalletSqliteDatabase::new((sql_path, pass)).await?,
                        None => bail!("Missing database password"),
                    }
                };

                Arc::new(sql)
            }
            "redb" => {
                #[cfg(feature = "redb")]
                {
                    let redb_path = work_dir.join("cdk-cli.redb");
                    Arc::new(WalletRedbDatabase::new(&redb_path)?)
                }
                #[cfg(not(feature = "redb"))]
                {
                    bail!("redb feature not enabled");
                }
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
            let mut rng = thread_rng();
            let random_bytes: [u8; 32] = rng.gen();

            let mnemonic = Mnemonic::from_entropy(&random_bytes)?;
            tracing::info!("Creating new seed");

            fs::write(seed_path, mnemonic.to_string())?;

            mnemonic
        }
    };
    let seed = mnemonic.to_seed_normalized("");

    // Parse currency unit from args
    let currency_unit = CurrencyUnit::from_str(&args.unit)
        .unwrap_or_else(|_| CurrencyUnit::Custom(args.unit.clone()));

    // Create MultiMintWallet with specified currency unit
    // The constructor will automatically load wallets for this currency unit
    let multi_mint_wallet = match &args.proxy {
        Some(proxy_url) => {
            MultiMintWallet::new_with_proxy(
                localstore.clone(),
                seed,
                currency_unit.clone(),
                proxy_url.clone(),
            )
            .await?
        }
        None => {
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            {
                match args.transport {
                    TorToggle::On => {
                        MultiMintWallet::new_with_tor(
                            localstore.clone(),
                            seed,
                            currency_unit.clone(),
                        )
                        .await?
                    }
                    TorToggle::Off => {
                        MultiMintWallet::new(localstore.clone(), seed, currency_unit.clone())
                            .await?
                    }
                }
            }
            #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
            {
                MultiMintWallet::new(localstore.clone(), seed, currency_unit.clone()).await?
            }
        }
    };

    match &args.command {
        Commands::DecodeToken(sub_command_args) => {
            sub_commands::decode_token::decode_token(sub_command_args)
        }
        Commands::Balance => sub_commands::balance::balance(&multi_mint_wallet).await,
        Commands::Melt(sub_command_args) => {
            sub_commands::melt::pay(&multi_mint_wallet, sub_command_args).await
        }
        Commands::Receive(sub_command_args) => {
            sub_commands::receive::receive(&multi_mint_wallet, sub_command_args, &work_dir).await
        }
        Commands::Send(sub_command_args) => {
            sub_commands::send::send(&multi_mint_wallet, sub_command_args).await
        }
        Commands::Transfer(sub_command_args) => {
            sub_commands::transfer::transfer(&multi_mint_wallet, sub_command_args).await
        }
        Commands::CheckPending => {
            sub_commands::check_pending::check_pending(&multi_mint_wallet).await
        }
        Commands::MintInfo(sub_command_args) => {
            sub_commands::mint_info::mint_info(args.proxy, sub_command_args).await
        }
        Commands::Mint(sub_command_args) => {
            sub_commands::mint::mint(&multi_mint_wallet, sub_command_args).await
        }
        Commands::MintPending => {
            sub_commands::pending_mints::mint_pending(&multi_mint_wallet).await
        }
        Commands::Burn(sub_command_args) => {
            sub_commands::burn::burn(&multi_mint_wallet, sub_command_args).await
        }
        Commands::Restore(sub_command_args) => {
            sub_commands::restore::restore(&multi_mint_wallet, sub_command_args).await
        }
        Commands::UpdateMintUrl(sub_command_args) => {
            sub_commands::update_mint_url::update_mint_url(&multi_mint_wallet, sub_command_args)
                .await
        }
        Commands::ListMintProofs => {
            sub_commands::list_mint_proofs::proofs(&multi_mint_wallet).await
        }
        Commands::DecodeRequest(sub_command_args) => {
            sub_commands::decode_request::decode_payment_request(sub_command_args)
        }
        Commands::PayRequest(sub_command_args) => {
            sub_commands::pay_request::pay_request(&multi_mint_wallet, sub_command_args).await
        }
        Commands::CreateRequest(sub_command_args) => {
            sub_commands::create_request::create_request(&multi_mint_wallet, sub_command_args).await
        }
        Commands::MintBlindAuth(sub_command_args) => {
            sub_commands::mint_blind_auth::mint_blind_auth(
                &multi_mint_wallet,
                sub_command_args,
                &work_dir,
            )
            .await
        }
        Commands::CatLogin(sub_command_args) => {
            sub_commands::cat_login::cat_login(&multi_mint_wallet, sub_command_args, &work_dir)
                .await
        }
        Commands::CatDeviceLogin(sub_command_args) => {
            sub_commands::cat_device_login::cat_device_login(
                &multi_mint_wallet,
                sub_command_args,
                &work_dir,
            )
            .await
        }
    }
}
