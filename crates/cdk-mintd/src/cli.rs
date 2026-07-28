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
        help = "Legacy startup flag; use a file: secret reference in persisted configuration",
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
    /// Initialize an unconfigured database from a TOML document.
    Init(ConfigFileArgs),
    /// Validate a TOML document without changing the database.
    Validate(ConfigFileArgs),
    /// Replace the configuration used by the next mintd start.
    Apply(ApplyConfigArgs),
    /// Print the stored configuration document.
    Show,
    /// Export the stored configuration document.
    Export(ConfigFileArgs),
}

/// Arguments containing a configuration document path.
#[derive(Debug, Args)]
pub struct ConfigFileArgs {
    /// TOML document to read or write.
    #[arg(long)]
    pub file: PathBuf,
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
        for command in ["init", "validate", "export"] {
            CLIArgs::try_parse_from(["cdk-mintd", "config", command, "--file", "/tmp/mint.toml"])
                .expect("configuration command should parse");
        }

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
    }

    #[test]
    fn no_subcommand_still_parses_daemon_startup() {
        let args = CLIArgs::try_parse_from(["cdk-mintd"]).expect("daemon arguments should parse");
        assert!(args.command.is_none());
    }
}
