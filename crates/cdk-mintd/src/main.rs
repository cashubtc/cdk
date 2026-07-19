//! CDK mint daemon and management command entry point.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
#[cfg(feature = "management-rpc")]
use cdk_mint_rpc::mint_rpc_cli::subcommands;
#[cfg(feature = "management-rpc")]
use cdk_mint_rpc::{
    ApplyConfigurationRequest, DiscardPendingConfigurationRequest, GetConfigurationRequest,
    GetInfoRequest,
};
use cdk_mintd::cli::{CLIArgs, Commands, ConfigCommands};
use cdk_mintd::config_service::ConfigurationService;
use cdk_mintd::{
    get_work_directory, initialize_configuration, open_offline_configuration_service,
    run_managed_mintd,
};
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let runtime = Arc::new(Runtime::new()?);
    let runtime_for_mint = Arc::clone(&runtime);

    runtime.block_on(async move {
        let args = CLIArgs::parse();
        reject_legacy_run_flags(&args)?;
        let work_dir = get_work_directory(&args).await?;

        #[cfg(feature = "sqlcipher")]
        let password = args.password.clone();
        #[cfg(not(feature = "sqlcipher"))]
        let password = None;

        let rpc_tls_dir = rpc_tls_directory(&args, &work_dir);
        let rpc_address = args.rpc_address.clone().unwrap_or_else(|| {
            let scheme = if rpc_tls_dir.is_some() {
                "https"
            } else {
                "http"
            };
            format!("{scheme}://127.0.0.1:8086")
        });
        let enable_logging = args.enable_logging;

        match args.command {
            Some(Commands::Config(config)) => {
                run_config_command(
                    config.command,
                    &work_dir,
                    password,
                    &rpc_address,
                    rpc_tls_dir.as_deref(),
                )
                .await
            }
            #[cfg(feature = "management-rpc")]
            Some(command) => {
                run_management_command(command, &rpc_address, rpc_tls_dir.as_deref()).await
            }
            None => {
                run_managed_mintd(
                    &work_dir,
                    password,
                    enable_logging,
                    Some(runtime_for_mint),
                    vec![],
                )
                .await
            }
        }
    })
}

fn reject_legacy_run_flags(args: &CLIArgs) -> Result<()> {
    if args.config.is_some() || args.seed_file.is_some() {
        bail!(
            "--config and --seed-file are not supported by any command; use `cdk-mintd config init --file <path>` once or `cdk-mintd config apply --file <path>` explicitly, and use file:/absolute/path for secret references"
        );
    }

    Ok(())
}

fn rpc_tls_directory(args: &CLIArgs, work_dir: &Path) -> Option<PathBuf> {
    args.rpc_tls_dir.clone().or_else(|| {
        let default_tls_dir = work_dir.join("tls");
        default_tls_dir.is_dir().then_some(default_tls_dir)
    })
}

async fn run_config_command(
    command: ConfigCommands,
    work_dir: &Path,
    password: Option<String>,
    rpc_address: &str,
    rpc_tls_dir: Option<&Path>,
) -> Result<()> {
    match command {
        ConfigCommands::Init(arguments) => {
            let document = read_configuration(&arguments.file)?;
            initialize_configuration(work_dir, &document, password).await?;
            println!(
                "Configuration initialized and staged. Start mintd to activate it authoritatively."
            );
            Ok(())
        }
        ConfigCommands::Validate(arguments) => {
            let document = read_configuration(&arguments.file)?;
            ConfigurationService::validate_document(&document)?;
            println!("Configuration is valid.");
            Ok(())
        }
        ConfigCommands::Apply(arguments) => {
            if arguments.offline {
                let document = read_configuration(&arguments.file)?;
                let service = open_offline_configuration_service(work_dir, password).await?;
                let outcome = service.apply(&document, arguments.validate_only).await?;
                if arguments.validate_only {
                    println!("Configuration is valid; no changes were persisted.");
                } else if outcome.restart_required {
                    println!("Configuration staged. Start mintd to activate it.");
                }
                return Ok(());
            }

            #[cfg(feature = "management-rpc")]
            {
                let document = read_configuration(&arguments.file)?;
                let mut client = cdk_mint_rpc::connect_client(rpc_address, rpc_tls_dir).await?;
                let response = client
                    .apply_configuration(ApplyConfigurationRequest {
                        config_toml: document,
                        validate_only: arguments.validate_only,
                    })
                    .await?
                    .into_inner();

                if arguments.validate_only {
                    println!("Configuration is valid; no changes were persisted.");
                } else if response.restart_required {
                    println!("Configuration staged. Restart mintd to activate it.");
                } else {
                    println!("Configuration applied.");
                }
                Ok(())
            }
            #[cfg(not(feature = "management-rpc"))]
            {
                let _ = (arguments, rpc_address, rpc_tls_dir);
                bail!("configuration apply requires the management-rpc feature")
            }
        }
        ConfigCommands::Show(arguments) => {
            if arguments.offline {
                let service = open_offline_configuration_service(work_dir, password).await?;
                let snapshot = service.snapshot().await?;
                print_configuration(snapshot.active, snapshot.pending);
                return Ok(());
            }

            #[cfg(feature = "management-rpc")]
            {
                let mut client = cdk_mint_rpc::connect_client(rpc_address, rpc_tls_dir).await?;
                let response = client
                    .get_configuration(GetConfigurationRequest {})
                    .await?
                    .into_inner();
                print_configuration(response.active_toml, response.pending_toml);
                Ok(())
            }
            #[cfg(not(feature = "management-rpc"))]
            {
                let _ = (rpc_address, rpc_tls_dir);
                bail!("configuration show requires the management-rpc feature")
            }
        }
        ConfigCommands::Export(arguments) => {
            if arguments.offline {
                let service = open_offline_configuration_service(work_dir, password).await?;
                let snapshot = service.snapshot().await?;
                write_export(&arguments.file, snapshot.active)?;
                return Ok(());
            }

            #[cfg(feature = "management-rpc")]
            {
                let mut client = cdk_mint_rpc::connect_client(rpc_address, rpc_tls_dir).await?;
                let response = client
                    .get_configuration(GetConfigurationRequest {})
                    .await?
                    .into_inner();
                write_export(&arguments.file, response.active_toml)?;
                Ok(())
            }
            #[cfg(not(feature = "management-rpc"))]
            {
                let _ = (arguments, rpc_address, rpc_tls_dir);
                bail!("configuration export requires the management-rpc feature")
            }
        }
        ConfigCommands::DiscardPending(arguments) => {
            if arguments.offline {
                let service = open_offline_configuration_service(work_dir, password).await?;
                service.discard_pending().await?;
                println!("Pending configuration discarded.");
                return Ok(());
            }

            #[cfg(feature = "management-rpc")]
            {
                let mut client = cdk_mint_rpc::connect_client(rpc_address, rpc_tls_dir).await?;
                client
                    .discard_pending_configuration(DiscardPendingConfigurationRequest {})
                    .await?;
                println!("Pending configuration discarded.");
                Ok(())
            }
            #[cfg(not(feature = "management-rpc"))]
            {
                let _ = (rpc_address, rpc_tls_dir);
                bail!("discarding pending configuration requires the management-rpc feature")
            }
        }
    }
}

fn read_configuration(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read configuration file {}", path.display()))
}

fn print_configuration(active: String, pending: Option<String>) {
    print!("{active}");
    if let Some(pending) = pending {
        println!("\n# Pending configuration (restart required)");
        print!("{pending}");
    }
}

fn write_export(path: &Path, document: String) -> Result<()> {
    std::fs::write(path, document)
        .with_context(|| format!("Failed to export configuration to {}", path.display()))?;
    println!("Configuration exported to {}.", path.display());
    Ok(())
}

#[cfg(feature = "management-rpc")]
async fn run_management_command(
    command: Commands,
    rpc_address: &str,
    rpc_tls_dir: Option<&Path>,
) -> Result<()> {
    let mut client = cdk_mint_rpc::connect_client(rpc_address, rpc_tls_dir).await?;

    match command {
        Commands::Config(_) => bail!("configuration command was dispatched incorrectly"),
        Commands::GetInfo => {
            let info = client.get_info(GetInfoRequest {}).await?.into_inner();
            println!(
                "name:             {}",
                info.name.as_deref().unwrap_or("None")
            );
            println!(
                "version:          {}",
                info.version.as_deref().unwrap_or("None")
            );
            println!(
                "description:      {}",
                info.description.as_deref().unwrap_or("None")
            );
            println!(
                "long description: {}",
                info.long_description.as_deref().unwrap_or("None")
            );
            println!("motd: {}", info.motd.as_deref().unwrap_or("None"));
            println!("icon_url: {}", info.icon_url.as_deref().unwrap_or("None"));
            println!("tos_url: {}", info.tos_url.as_deref().unwrap_or("None"));
            for url in info.urls {
                println!("mint_url: {url}");
            }
            for contact in info.contact {
                println!("method: {}, info: {}", contact.method, contact.info);
            }
            println!("total issued:     {} sat", info.total_issued);
            println!("total redeemed:   {} sat", info.total_redeemed);
        }
        Commands::UpdateMotd(arguments) => {
            subcommands::update_motd(&mut client, &arguments).await?;
        }
        Commands::UpdateShortDescription(arguments) => {
            subcommands::update_short_description(&mut client, &arguments).await?;
        }
        Commands::UpdateLongDescription(arguments) => {
            subcommands::update_long_description(&mut client, &arguments).await?;
        }
        Commands::UpdateName(arguments) => {
            subcommands::update_name(&mut client, &arguments).await?;
        }
        Commands::UpdateIconUrl(arguments) => {
            subcommands::update_icon_url(&mut client, &arguments).await?;
        }
        Commands::UpdateTosUrl(arguments) => {
            subcommands::update_tos_url(&mut client, &arguments).await?;
        }
        Commands::AddUrl(arguments) => {
            subcommands::add_url(&mut client, &arguments).await?;
        }
        Commands::RemoveUrl(arguments) => {
            subcommands::remove_url(&mut client, &arguments).await?;
        }
        Commands::AddContact(arguments) => {
            subcommands::add_contact(&mut client, &arguments).await?;
        }
        Commands::RemoveContact(arguments) => {
            subcommands::remove_contact(&mut client, &arguments).await?;
        }
        Commands::UpdateNut04(arguments) => {
            subcommands::update_nut04(&mut client, &arguments).await?;
        }
        Commands::UpdateNut05(arguments) => {
            subcommands::update_nut05(&mut client, &arguments).await?;
        }
        Commands::UpdateQuoteTtl(arguments) => {
            subcommands::update_quote_ttl(&mut client, &arguments).await?;
        }
        Commands::GetQuoteTtl => {
            subcommands::get_quote_ttl(&mut client).await?;
        }
        Commands::UpdateNut04QuoteState(arguments) => {
            subcommands::update_nut04_quote_state(&mut client, &arguments).await?;
        }
        Commands::RotateNextKeyset(arguments) => {
            subcommands::rotate_next_keyset(&mut client, &arguments).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_flags_are_rejected_before_config_subcommands() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "--config",
            "/tmp/legacy.toml",
            "config",
            "validate",
            "--file",
            "/tmp/config.toml",
        ])
        .expect("arguments should parse for an explicit migration error");

        let error = reject_legacy_run_flags(&args).expect_err("legacy flag must be rejected");
        assert!(error.to_string().contains("not supported by any command"));
    }

    #[test]
    fn legacy_flags_are_rejected_after_config_subcommands() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "validate",
            "--file",
            "/tmp/config.toml",
            "--seed-file",
            "/tmp/legacy-seed",
        ])
        .expect("global legacy flag should parse for an explicit migration error");

        let error = reject_legacy_run_flags(&args).expect_err("legacy flag must be rejected");
        assert!(error.to_string().contains("not supported by any command"));
    }
}
