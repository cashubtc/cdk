use std::path::PathBuf;

use anyhow::{anyhow, Result};
use cdk_mint_rpc::cdk_mint_management_client::CdkMintManagementClient;
use cdk_mint_rpc::cdk_mint_reporting_client::CdkMintReportingClient;
use cdk_mint_rpc::mint_rpc_cli::subcommands;
use clap::{Parser, Subcommand};
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
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
    /// Update terms of service url
    UpdateTosUrl(subcommands::UpdateTosUrlCommand),
    /// Add Url
    AddUrl(subcommands::AddUrlCommand),
    /// Remove Url
    RemoveUrl(subcommands::RemoveUrlCommand),
    /// Add contact
    AddContact(subcommands::AddContactCommand),
    /// Remove contact
    RemoveContact(subcommands::RemoveContactCommand),
    /// Update nut04
    UpdateNut04(subcommands::UpdateNut04Command),
    /// Update nut05
    UpdateNut05(subcommands::UpdateNut05Command),
    /// Update quote ttl
    UpdateQuoteTtl(subcommands::UpdateQuoteTtlCommand),
    /// Get quote ttl
    GetQuoteTtl,
    /// Update Nut04 quote
    UpdateNut04QuoteState(subcommands::UpdateNut04QuoteCommand),
    /// Rotate next keyset
    RotateNextKeyset(subcommands::RotateNextKeysetCommand),
    /// Get info
    GetInfo,
    /// Get balances
    GetBalances(subcommands::GetBalancesCommand),
    /// Get keysets
    GetKeysets(subcommands::GetKeysetsCommand),
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
            let home_dir = home::home_dir().ok_or(anyhow!("Could not find home dir"))?;

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

    let mut management_client = CdkMintManagementClient::new(channel.clone());
    let mut reporting_client = CdkMintReportingClient::new(channel);

    match cli.command {
        Commands::UpdateMotd(sub_command_args) => {
            subcommands::update_motd(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateShortDescription(sub_command_args) => {
            subcommands::update_short_description(&mut management_client, &sub_command_args)
                .await?;
        }
        Commands::UpdateLongDescription(sub_command_args) => {
            subcommands::update_long_description(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateName(sub_command_args) => {
            subcommands::update_name(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateIconUrl(sub_command_args) => {
            subcommands::update_icon_url(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateTosUrl(sub_command_args) => {
            subcommands::update_tos_url(&mut management_client, &sub_command_args).await?;
        }
        Commands::AddUrl(sub_command_args) => {
            subcommands::add_url(&mut management_client, &sub_command_args).await?;
        }
        Commands::RemoveUrl(sub_command_args) => {
            subcommands::remove_url(&mut management_client, &sub_command_args).await?;
        }
        Commands::AddContact(sub_command_args) => {
            subcommands::add_contact(&mut management_client, &sub_command_args).await?;
        }
        Commands::RemoveContact(sub_command_args) => {
            subcommands::remove_contact(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateNut04(sub_command_args) => {
            subcommands::update_nut04(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateNut05(sub_command_args) => {
            subcommands::update_nut05(&mut management_client, &sub_command_args).await?;
        }
        Commands::GetQuoteTtl => {
            subcommands::get_quote_ttl(&mut management_client).await?;
        }
        Commands::UpdateQuoteTtl(sub_command_args) => {
            subcommands::update_quote_ttl(&mut management_client, &sub_command_args).await?;
        }
        Commands::UpdateNut04QuoteState(sub_command_args) => {
            subcommands::update_nut04_quote_state(&mut management_client, &sub_command_args)
                .await?;
        }
        Commands::RotateNextKeyset(sub_command_args) => {
            subcommands::rotate_next_keyset(&mut management_client, &sub_command_args).await?;
        }
        Commands::GetInfo => {
            subcommands::get_info(&mut reporting_client).await?;
        }
        Commands::GetBalances(sub_command_args) => {
            subcommands::get_balances(&mut reporting_client, &sub_command_args).await?;
        }
        Commands::GetKeysets(sub_command_args) => {
            subcommands::get_keysets(&mut reporting_client, &sub_command_args).await?;
        }
    }

    Ok(())
}
