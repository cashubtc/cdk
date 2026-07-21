//! CDK MINTD

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cdk_mintd::cli::{CLIArgs, Commands, ConfigCommands};
use cdk_mintd::get_work_directory;
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = CLIArgs::parse();
        if args.config.is_some() || args.seed_file.is_some() {
            bail!(
                "--config and --seed-file are no longer startup inputs; import a TOML document with `cdk-mintd config init --file <path>` or replace it with `cdk-mintd config apply --file <path>`"
            );
        }
        let work_dir = if matches!(
            &args.command,
            Some(Commands::Config(config))
                if matches!(&config.command, ConfigCommands::Validate(_))
        ) {
            None
        } else {
            Some(get_work_directory(&args).await?)
        };

        #[cfg(feature = "sqlcipher")]
        let password = args.password.clone();

        #[cfg(not(feature = "sqlcipher"))]
        let password = None;

        match args.command {
            Some(Commands::Config(config)) => match config.command {
                ConfigCommands::Init(file) => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    let document = read_document(&file.file)?;
                    cdk_mintd::initialize_configuration(work_dir, &document, password).await?;
                    println!("Configuration initialized. Start cdk-mintd to apply it.");
                    Ok(())
                }
                ConfigCommands::Validate(file) => {
                    let document = read_document(&file.file)?;
                    cdk_mintd::validate_configuration_document(&document).await?;
                    println!("Configuration is valid.");
                    Ok(())
                }
                ConfigCommands::Apply(apply) => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    let document = read_document(&apply.file)?;
                    cdk_mintd::apply_configuration(
                        work_dir,
                        &document,
                        apply.validate_only,
                        password,
                    )
                    .await?;
                    if apply.validate_only {
                        println!("Configuration is valid and was not changed.");
                    } else {
                        println!("Configuration replaced. Restart cdk-mintd to apply it.");
                    }
                    Ok(())
                }
                ConfigCommands::Show => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    let document = cdk_mintd::stored_configuration_document(work_dir, password)
                        .await?;
                    print!("{document}");
                    Ok(())
                }
                ConfigCommands::Export(file) => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    let document = cdk_mintd::stored_configuration_document(work_dir, password)
                        .await?;
                    std::fs::write(&file.file, document).with_context(|| {
                        format!("could not export configuration to {}", file.file.display())
                    })?;
                    println!("Configuration exported to {}.", file.file.display());
                    Ok(())
                }
            },
            None => {
                let work_dir = work_dir
                    .as_deref()
                    .expect("daemon startup has a work directory");
                cdk_mintd::run_mintd_from_database(
                    work_dir,
                    password,
                    args.enable_logging,
                    Some(rt_clone),
                    vec![],
                )
                .await
            }
        }
    })
}

fn read_document(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("could not read configuration document {}", path.display()))
}
