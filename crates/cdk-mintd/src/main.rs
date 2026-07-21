//! CDK mint daemon and configuration command entry point.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cdk_mintd::cli::{CLIArgs, Commands, ConfigCommands};
use cdk_mintd::config_service::ConfigurationService;
use cdk_mintd::{
    get_work_directory, initialize_configuration, open_direct_configuration_service,
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

        let enable_logging = args.enable_logging;

        match args.command {
            Some(Commands::Config(config)) => {
                run_config_command(config.command, &work_dir, password).await
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

async fn run_config_command(
    command: ConfigCommands,
    work_dir: &Path,
    password: Option<String>,
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
            let document = read_configuration(&arguments.file)?;
            let service = open_direct_configuration_service(work_dir, password).await?;
            let outcome = service.apply(&document, arguments.validate_only).await?;
            if arguments.validate_only {
                println!("Configuration is valid; no changes were persisted.");
            } else if outcome.restart_required {
                println!("Configuration staged. Start mintd to activate it.");
            }
            Ok(())
        }
        ConfigCommands::Show => {
            let service = open_direct_configuration_service(work_dir, password).await?;
            let snapshot = service.snapshot().await?;
            print_configuration(snapshot.active, snapshot.pending);
            Ok(())
        }
        ConfigCommands::Export(arguments) => {
            let service = open_direct_configuration_service(work_dir, password).await?;
            let snapshot = service.snapshot().await?;
            write_export(&arguments.file, snapshot.active)?;
            Ok(())
        }
        ConfigCommands::DiscardPending => {
            let service = open_direct_configuration_service(work_dir, password).await?;
            service.discard_pending().await?;
            println!("Pending configuration discarded.");
            Ok(())
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
        .expect("arguments should parse");

        let error = reject_legacy_run_flags(&args).expect_err("legacy flags must be rejected");
        assert!(error.to_string().contains("--config"));
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
            "/tmp/seed",
        ])
        .expect("arguments should parse");

        let error = reject_legacy_run_flags(&args).expect_err("legacy flags must be rejected");
        assert!(error.to_string().contains("--seed-file"));
    }
}
