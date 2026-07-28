use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(about = "A cashu mint written in rust", author = env!("CARGO_PKG_AUTHORS"), version = env!("CARGO_PKG_VERSION"))]
pub struct CLIArgs {
    #[arg(
        short,
        long,
        help = "Use the <directory> as the location of the database",
        required = false
    )]
    pub work_dir: Option<PathBuf>,
    #[cfg(feature = "sqlcipher")]
    #[arg(
        short,
        long,
        global = true,
        help = "Database password for SQLCipher (required when opening an encrypted database)"
    )]
    pub password: Option<String>,
    #[arg(
        short,
        long,
        global = true,
        help = "Legacy startup flag; use `config init` or `config apply` instead",
        required = false
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Legacy seed file; accepted only by `config migrate`",
        required = false
    )]
    pub seed_file: Option<PathBuf>,
    #[arg(
        long,
        help = "Enable logging output",
        required = false,
        action = clap::ArgAction::SetTrue,
        default_value = "true"
    )]
    pub enable_logging: bool,
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Commands exposed by the `cdk-mintd` binary.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Manage the database-backed mintd configuration.
    Config(ConfigArgs),
}

/// Arguments for database-backed configuration management.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

/// Database-backed configuration operations.
#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Convert a legacy TOML plus environment overrides into an import document.
    Migrate(MigrateConfigArgs),
    /// Initialize an unconfigured database from a TOML document.
    Init(ConfigFileArgs),
    /// Validate a TOML document without changing the database.
    Validate(ConfigFileArgs),
    /// Replace the configuration used by the next mintd start.
    Apply(ApplyConfigArgs),
    /// Print the stored configuration document.
    Show,
    /// Export the stored configuration document.
    Export(ExportConfigArgs),
}

/// Arguments for migrating a legacy configuration.
#[derive(Debug, Args)]
pub struct MigrateConfigArgs {
    /// Legacy TOML document to read.
    #[arg(long)]
    pub file: PathBuf,
    /// Migrated TOML document to write.
    #[arg(long)]
    pub output: PathBuf,
    /// Directory for literal secrets extracted from the legacy TOML.
    #[arg(long)]
    pub secrets_dir: Option<PathBuf>,
    /// Overwrite generated output and secret files.
    #[arg(long)]
    pub force: bool,
}

/// Arguments containing a configuration document path.
#[derive(Debug, Args)]
pub struct ConfigFileArgs {
    /// TOML document to read or write.
    #[arg(long)]
    pub file: PathBuf,
}

/// Arguments for exporting the stored configuration.
#[derive(Debug, Args)]
pub struct ExportConfigArgs {
    /// TOML document to write.
    #[arg(long)]
    pub file: PathBuf,
    /// Overwrite the destination if it already exists.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for replacing the stored configuration.
#[derive(Debug, Args)]
pub struct ApplyConfigArgs {
    /// TOML document to validate and store.
    #[arg(long)]
    pub file: PathBuf,
    /// Validate the document and persisted constraints without writing it.
    #[arg(long)]
    pub validate_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configuration_commands() {
        for command in ["init", "validate"] {
            CLIArgs::try_parse_from(["cdk-mintd", "config", command, "--file", "/tmp/mint.toml"])
                .expect("configuration command should parse");
        }

        let args =
            CLIArgs::try_parse_from(["cdk-mintd", "config", "export", "--file", "/tmp/mint.toml"])
                .expect("configuration export should parse");
        assert!(matches!(
            args.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommands::Export(ExportConfigArgs { force: false, .. }),
            }))
        ));

        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "export",
            "--file",
            "/tmp/mint.toml",
            "--force",
        ])
        .expect("configuration export should parse");
        assert!(matches!(
            args.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommands::Export(ExportConfigArgs { force: true, .. }),
            }))
        ));

        CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "apply",
            "--file",
            "/tmp/mint.toml",
            "--validate-only",
        ])
        .expect("configuration apply should parse");
        CLIArgs::try_parse_from(["cdk-mintd", "config", "show"])
            .expect("configuration show should parse");

        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "migrate",
            "--file",
            "/tmp/legacy.toml",
            "--output",
            "/tmp/migrated.toml",
            "--secrets-dir",
            "/tmp/mint-secrets",
        ])
        .expect("configuration migration should parse");
        assert!(matches!(
            args.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommands::Migrate(MigrateConfigArgs { force: false, .. }),
            }))
        ));

        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "--seed-file",
            "/tmp/seed.txt",
            "config",
            "migrate",
            "--file",
            "/tmp/legacy.toml",
            "--output",
            "/tmp/migrated.toml",
        ])
        .expect("legacy seed-file migration should parse");
        assert_eq!(args.seed_file, Some(PathBuf::from("/tmp/seed.txt")));
    }

    #[test]
    fn no_subcommand_still_parses_daemon_startup() {
        let args = CLIArgs::try_parse_from(["cdk-mintd"]).expect("daemon arguments should parse");
        assert!(args.command.is_none());
    }
}
