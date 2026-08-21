//! Mint RPC CLI

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use cdk_common::grpc::{VersionInterceptor, VERSION_HEADER};
use cdk_mint_rpc::cdk_mint_client::CdkMintClient;
use cdk_mint_rpc::keyset::keyset_service_client::KeysetServiceClient;
use cdk_mint_rpc::mint_rpc_cli::subcommands;
use cdk_mint_rpc::payment_method::payment_method_service_client::PaymentMethodServiceClient;
use cdk_mint_rpc::quote::quote_service_client::QuoteServiceClient;
use cdk_mint_rpc::wallet::wallet_service_client::WalletServiceClient;
use cdk_mint_rpc::GetInfoRequest;
use clap::{Parser, Subcommand};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::Request;
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
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get info
    GetInfo,
    /// Update motd
    UpdateMotd(subcommands::UpdateMotdCommand),
    /// Update short description
    UpdateShortDescription(subcommands::UpdateShortDescriptionCommand),
    /// Update long description
    UpdateLongDescription(subcommands::UpdateLongDescriptionCommand),
    /// Update name
    UpdateName(subcommands::UpdateNameCommand),
    /// Update icon url
    UpdateIconUrl(subcommands::UpdateIconUrlCommand),
    /// Update terms of service URL
    UpdateTosUrl(subcommands::UpdateTosUrlCommand),
    /// Add Url
    AddUrl(subcommands::AddUrlCommand),
    /// Remove Url
    RemoveUrl(subcommands::RemoveUrlCommand),
    /// Add contact
    AddContact(subcommands::AddContactCommand),
    /// Remove contact
    RemoveContact(subcommands::RemoveContactCommand),
    /// Update mint (NUT-04) payment method settings
    #[command(alias = "update-nut04")]
    UpdateMintMethod(subcommands::UpdateMintMethodCommand),
    /// Update melt (NUT-05) payment method settings
    #[command(alias = "update-nut05")]
    UpdateMeltMethod(subcommands::UpdateMeltMethodCommand),
    /// Enable or disable minting and melting
    UpdateDisabled(subcommands::UpdateDisabledCommand),
    /// Update quote ttl
    UpdateQuoteTtl(subcommands::UpdateQuoteTtlCommand),
    /// Get quote ttl
    GetQuoteTtl,
    /// Update mint quote state
    #[command(alias = "update-nut04-quote-state")]
    UpdateMintQuoteState(subcommands::UpdateMintQuoteStateCommand),
    /// Rotate next keyset
    RotateNextKeyset(subcommands::RotateNextKeysetCommand),
    /// Get the BDK on-chain wallet balance
    GetWalletBalance,
    /// Create an on-chain address for operator wallet deposits
    CreateWalletDepositAddress,
    /// List BDK on-chain wallet transactions
    ListWalletTransactions(subcommands::WalletPaginationCommand),
    /// List addresses revealed by the BDK on-chain wallet
    ListWalletAddresses(subcommands::WalletPaginationCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Cli = Cli::parse();

    // Initialize logging based on CLI arguments
    init_logging(args.common.enable_logging, args.common.log_level);

    let cli = Cli::parse();

    let work_dir = match &args.work_dir {
        Some(work_dir) => work_dir.clone(),
        None => {
            let home_dir =
                cdk_common::util::home_dir().ok_or(anyhow!("Could not find home dir"))?;

            home_dir.join(DEFAULT_WORK_DIR)
        }
    };

    std::fs::create_dir_all(&work_dir)?;
    tracing::debug!("Using work dir: {}", work_dir.display());

    let channel = if work_dir.join("tls").is_dir() {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }

        // TLS directory exists, configure TLS
        let server_root_ca_cert = std::fs::read_to_string(work_dir.join("tls/ca.pem"))?;
        let server_root_ca_cert = Certificate::from_pem(server_root_ca_cert);
        let client_cert = std::fs::read_to_string(work_dir.join("tls/client.pem"))?;
        let client_key = std::fs::read_to_string(work_dir.join("tls/client.key"))?;
        let client_identity = Identity::from_pem(client_cert, client_key);
        let tls = ClientTlsConfig::new()
            .ca_certificate(server_root_ca_cert)
            .identity(client_identity);

        Channel::from_shared(cli.addr.to_string())?
            .tls_config(tls)?
            .connect()
            .await?
    } else {
        // No TLS directory, skip TLS configuration
        Channel::from_shared(cli.addr.to_string())?
            .connect()
            .await?
    };

    // Shared version header interceptor
    let interceptor =
        VersionInterceptor::new(VERSION_HEADER, cdk_common::MINT_RPC_PROTOCOL_VERSION);
    let mut client = CdkMintClient::with_interceptor(channel.clone(), interceptor.clone());
    let mut wallet_client =
        WalletServiceClient::with_interceptor(channel.clone(), interceptor.clone());

    match cli.command {
        Commands::GetInfo => {
            let response = client.get_info(Request::new(GetInfoRequest {})).await?;
            let info = response.into_inner();
            println!(
                "name:             {}",
                info.name.unwrap_or("None".to_string())
            );
            println!(
                "version:          {}",
                info.version.unwrap_or("None".to_string())
            );
            println!(
                "description:      {}",
                info.description.unwrap_or("None".to_string())
            );
            println!(
                "long description: {}",
                info.long_description.unwrap_or("None".to_string())
            );
            println!("motd: {}", info.motd.unwrap_or("None".to_string()));
            println!("icon_url: {}", info.icon_url.unwrap_or("None".to_string()));
            println!("tos_url: {}", info.tos_url.unwrap_or("None".to_string()));

            for url in info.urls {
                println!("mint_url: {url}");
            }

            for contact in info.contact {
                println!("method: {}, info: {}", contact.method, contact.info);
            }
            println!("total issued:     {} sat", info.total_issued);
            println!("total redeemed:   {} sat", info.total_redeemed);
        }
        Commands::UpdateMotd(sub_command_args) => {
            subcommands::update_motd(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateShortDescription(sub_command_args) => {
            subcommands::update_short_description(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateLongDescription(sub_command_args) => {
            subcommands::update_long_description(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateName(sub_command_args) => {
            subcommands::update_name(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateIconUrl(sub_command_args) => {
            subcommands::update_icon_url(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateTosUrl(sub_command_args) => {
            subcommands::update_tos_url(&mut client, &sub_command_args).await?;
        }
        Commands::AddUrl(sub_command_args) => {
            subcommands::add_url(&mut client, &sub_command_args).await?;
        }
        Commands::RemoveUrl(sub_command_args) => {
            subcommands::remove_url(&mut client, &sub_command_args).await?;
        }
        Commands::AddContact(sub_command_args) => {
            subcommands::add_contact(&mut client, &sub_command_args).await?;
        }
        Commands::RemoveContact(sub_command_args) => {
            subcommands::remove_contact(&mut client, &sub_command_args).await?;
        }
        Commands::UpdateMintMethod(sub_command_args) => {
            let mut payment_method_client =
                PaymentMethodServiceClient::with_interceptor(channel, interceptor);
            subcommands::update_mint_method(&mut payment_method_client, &sub_command_args).await?;
        }
        Commands::UpdateMeltMethod(sub_command_args) => {
            let mut payment_method_client =
                PaymentMethodServiceClient::with_interceptor(channel, interceptor);
            subcommands::update_melt_method(&mut payment_method_client, &sub_command_args).await?;
        }
        Commands::UpdateDisabled(sub_command_args) => {
            let mut payment_method_client =
                PaymentMethodServiceClient::with_interceptor(channel, interceptor);
            subcommands::update_disabled(&mut payment_method_client, &sub_command_args).await?;
        }
        Commands::GetQuoteTtl => {
            let mut quote_client = QuoteServiceClient::with_interceptor(channel, interceptor);
            subcommands::get_quote_ttl(&mut quote_client).await?;
        }
        Commands::UpdateQuoteTtl(sub_command_args) => {
            let mut quote_client = QuoteServiceClient::with_interceptor(channel, interceptor);
            subcommands::update_quote_ttl(&mut quote_client, &sub_command_args).await?;
        }
        Commands::UpdateMintQuoteState(sub_command_args) => {
            let mut quote_client = QuoteServiceClient::with_interceptor(channel, interceptor);
            subcommands::update_mint_quote_state(&mut quote_client, &sub_command_args).await?;
        }
        Commands::RotateNextKeyset(sub_command_args) => {
            let mut keyset_client = KeysetServiceClient::with_interceptor(channel, interceptor);
            subcommands::rotate_next_keyset(&mut keyset_client, &sub_command_args).await?;
        }
        Commands::GetWalletBalance => {
            subcommands::get_wallet_balance(&mut wallet_client).await?;
        }
        Commands::CreateWalletDepositAddress => {
            subcommands::create_wallet_deposit_address(&mut wallet_client).await?;
        }
        Commands::ListWalletTransactions(args) => {
            subcommands::list_wallet_transactions(&mut wallet_client, &args).await?;
        }
        Commands::ListWalletAddresses(args) => {
            subcommands::list_wallet_addresses(&mut wallet_client, &args).await?;
        }
    }

    Ok(())
}
