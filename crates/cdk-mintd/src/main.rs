//! CDK MINTD

use std::io::Write;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cdk_mintd::cli::{CLIArgs, Commands, ConfigCommands};
use cdk_mintd::{get_work_directory, BdkWalletPolicy, MintInitializationMode};
use clap::Parser;
use tokio::runtime::Runtime;

fn main() -> Result<()> {
    let rt = Arc::new(Runtime::new()?);

    let rt_clone = Arc::clone(&rt);

    rt.block_on(async {
        let args = CLIArgs::parse();
        let is_migration = matches!(
            &args.command,
            Some(Commands::Config(config))
                if matches!(&config.command, ConfigCommands::Migrate(_))
        );
        if args.config.is_some() || (args.seed_file.is_some() && !is_migration) {
            bail!(
                "--config and --seed-file are no longer startup inputs; migrate a legacy document with `cdk-mintd config migrate --file <old> --output <new>`, import one with `cdk-mintd config init --existing-mint --file <path>`, initialize a new mint with `cdk-mintd config init --new-mint --file <path>`, or replace it with `cdk-mintd config apply --file <path>`"
            );
        }
        let work_dir = if matches!(
            &args.command,
            Some(Commands::Config(config))
                if matches!(
                    &config.command,
                    ConfigCommands::Validate(_) | ConfigCommands::Migrate(_)
                )
        ) {
            None
        } else {
            Some(get_work_directory(&args).await?)
        };

        #[cfg(feature = "sqlcipher")]
        let password = args.password.clone();

        #[cfg(not(feature = "sqlcipher"))]
        let password = None;
        let legacy_seed_file = args.seed_file.clone();

        match args.command {
            Some(Commands::Config(config)) => match config.command {
                ConfigCommands::Migrate(migrate) => {
                    let outcome = cdk_mintd::migrate_legacy_configuration(
                        &migrate.file,
                        &migrate.output,
                        migrate.secrets_dir.as_deref(),
                        legacy_seed_file.as_deref(),
                        migrate.force,
                    )?;
                    println!(
                        "Migrated configuration written to {}.",
                        outcome.output.display()
                    );
                    if let Some(secrets_dir) = outcome.secrets_dir {
                        println!(
                            "Extracted {} literal secret(s) to {}.",
                            outcome.secret_files_written,
                            secrets_dir.display()
                        );
                    }
                    println!(
                        "Review it, then run `cdk-mintd config validate --file {}`.",
                        outcome.output.display()
                    );
                    Ok(())
                }
                ConfigCommands::Init(init) => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    let document = read_document(&init.file)?;
                    let mode = match (init.new_mint, init.existing_mint) {
                        (true, false) => MintInitializationMode::New,
                        (false, true) => MintInitializationMode::Existing,
                        _ => bail!("exactly one mint initialization mode is required"),
                    };
                    let bdk_wallet_policy = match init.allow_new_bdk_wallet {
                        true => BdkWalletPolicy::AllowNew,
                        false => BdkWalletPolicy::RequireExisting,
                    };
                    cdk_mintd::initialize_configuration(
                        work_dir,
                        &document,
                        mode,
                        bdk_wallet_policy,
                        password,
                    )
                    .await?;
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
                    let bdk_wallet_policy = match apply.allow_new_bdk_wallet {
                        true => BdkWalletPolicy::AllowNew,
                        false => BdkWalletPolicy::RequireExisting,
                    };
                    cdk_mintd::apply_configuration(
                        work_dir,
                        &document,
                        apply.validate_only,
                        bdk_wallet_policy,
                        password,
                    )
                    .await?;
                    if apply.validate_only {
                        println!("Configuration is valid and was not changed.");
                    } else {
                        println!("Configuration staged. Restart cdk-mintd to apply it.");
                    }
                    Ok(())
                }
                ConfigCommands::Rollback => {
                    let work_dir = work_dir
                        .as_deref()
                        .expect("database commands have a work directory");
                    cdk_mintd::rollback_configuration(work_dir, password).await?;
                    println!(
                        "Previous applied configuration staged. Restart cdk-mintd to activate it."
                    );
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
                    export_document(&file.file, &document, file.force)?;
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

fn export_document(path: &std::path::Path, document: &str, force: bool) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    set_export_creation_mode(&mut options);

    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if !force && error.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "configuration export destination {} already exists; pass --force to overwrite it",
                path.display()
            );
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not export configuration to {}", path.display()));
        }
    };

    file.write_all(document.as_bytes())
        .with_context(|| format!("could not export configuration to {}", path.display()))
}

#[cfg(unix)]
fn set_export_creation_mode(options: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_export_creation_mode(_options: &mut std::fs::OpenOptions) {}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn export_test_directory(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should follow the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cdk-mintd-export-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn export_requires_force_to_replace_existing_file() {
        let directory = export_test_directory("replace");
        std::fs::create_dir(&directory).expect("create test directory");
        let path = directory.join("config.toml");
        std::fs::write(&path, "existing").expect("create existing export");

        let error =
            export_document(&path, "replacement", false).expect_err("existing file should fail");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read unchanged export"),
            "existing"
        );

        export_document(&path, "replacement", true).expect("forced export should replace file");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read replacement export"),
            "replacement"
        );

        std::fs::remove_dir_all(directory).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn export_uses_owner_only_permissions_for_new_files() {
        use std::os::unix::fs::PermissionsExt;

        let directory = export_test_directory("permissions");
        std::fs::create_dir(&directory).expect("create test directory");
        let path = directory.join("config.toml");

        export_document(&path, "first", false).expect("create export");
        assert_eq!(
            std::fs::metadata(&path)
                .expect("read export metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        std::fs::remove_dir_all(directory).expect("remove test directory");
    }
}
