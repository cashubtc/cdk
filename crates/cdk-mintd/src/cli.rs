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
        help = "Database password for SQLCipher (required only when opening the local database)"
    )]
    pub password: Option<String>,
    #[arg(
        short,
        long,
        global = true,
        help = "Legacy startup flag (rejected by the binary); use `config init/apply --file`",
        required = false
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Legacy startup flag (rejected by the binary); use a file: secret reference",
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
    /// Manage the persisted mint daemon configuration.
    Config(ConfigArgs),
}

/// Arguments for configuration management commands.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

/// Persisted configuration management operations.
///
/// These commands access the configuration database directly. Immediate mint
/// management over RPC is provided by `cdk-mint-cli`, not this binary.
#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Initialize an unconfigured mint database from a configuration file.
    Init(ConfigFileArgs),
    /// Validate a configuration file without changing persisted configuration.
    Validate(ConfigFileArgs),
    /// Apply a configuration file to the database (stages a restart-bound update).
    Apply(ApplyConfigArgs),
    /// Show the effective persisted configuration.
    Show,
    /// Export the effective persisted configuration to a file.
    Export(ExportConfigArgs),
    /// Discard a staged configuration that has not yet been activated.
    DiscardPending,
}

/// Arguments for a command that reads a configuration file.
#[derive(Debug, Args)]
pub struct ConfigFileArgs {
    /// Configuration file to read.
    #[arg(long)]
    pub file: PathBuf,
}

/// Arguments for applying a configuration file.
#[derive(Debug, Args)]
pub struct ApplyConfigArgs {
    /// Configuration file to apply.
    #[arg(long)]
    pub file: PathBuf,
    /// Validate the configuration without persisting it.
    #[arg(long)]
    pub validate_only: bool,
}

/// Arguments for exporting persisted configuration.
#[derive(Debug, Args)]
pub struct ExportConfigArgs {
    /// File to write the exported configuration to.
    #[arg(long)]
    pub file: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_preserves_run_compatibility() {
        let args = CLIArgs::try_parse_from(["cdk-mintd"]).expect("arguments should parse");

        assert!(args.command.is_none());
        assert!(args.enable_logging);
    }

    #[test]
    fn legacy_run_flags_parse_for_explicit_rejection() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "--work-dir",
            "/tmp/cdk-mintd",
            "--config",
            "/tmp/config.toml",
            "--seed-file",
            "/tmp/seed",
        ])
        .expect("arguments should parse");

        assert!(args.command.is_none());
        assert_eq!(args.work_dir, Some(PathBuf::from("/tmp/cdk-mintd")));
        assert_eq!(args.config, Some(PathBuf::from("/tmp/config.toml")));
        assert_eq!(args.seed_file, Some(PathBuf::from("/tmp/seed")));
    }

    #[test]
    fn legacy_run_flags_are_recognized_after_subcommands() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "validate",
            "--file",
            "/tmp/config.toml",
            "--config",
            "/tmp/legacy.toml",
        ])
        .expect("legacy flag should parse so the binary can reject it explicitly");

        assert_eq!(args.config, Some(PathBuf::from("/tmp/legacy.toml")));
        assert!(matches!(args.command, Some(Commands::Config(_))));
    }

    #[cfg(feature = "sqlcipher")]
    #[test]
    fn sqlcipher_password_is_not_a_parse_time_requirement() {
        let validate = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "validate",
            "--file",
            "/tmp/config.toml",
        ])
        .expect("local validation must parse without a database password");
        assert!(validate.password.is_none());

        let direct =
            CLIArgs::try_parse_from(["cdk-mintd", "config", "show", "--password", "secret"])
                .expect("direct database password should be accepted after the subcommand");
        assert_eq!(direct.password.as_deref(), Some("secret"));
    }

    #[test]
    fn parses_config_init() {
        let args =
            CLIArgs::try_parse_from(["cdk-mintd", "config", "init", "--file", "/tmp/config.toml"])
                .expect("arguments should parse");

        let Some(Commands::Config(config)) = args.command else {
            panic!("expected config command");
        };
        let ConfigCommands::Init(init) = config.command else {
            panic!("expected config init command");
        };
        assert_eq!(init.file, PathBuf::from("/tmp/config.toml"));
    }

    #[test]
    fn parses_config_apply_validate_only() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "apply",
            "--file",
            "/tmp/config.toml",
            "--validate-only",
        ])
        .expect("arguments should parse");

        let Some(Commands::Config(config)) = args.command else {
            panic!("expected config command");
        };
        let ConfigCommands::Apply(apply) = config.command else {
            panic!("expected config apply command");
        };
        assert_eq!(apply.file, PathBuf::from("/tmp/config.toml"));
        assert!(apply.validate_only);
    }

    #[test]
    fn removed_rpc_transport_flags_are_rejected() {
        for args in [
            vec![
                "cdk-mintd",
                "config",
                "apply",
                "--file",
                "/tmp/config.toml",
                "--rpc",
                "http://127.0.0.1:8086",
            ],
            vec!["cdk-mintd", "--rpc-address", "http://127.0.0.1:8086"],
            vec!["cdk-mintd", "--rpc-tls-dir", "/tmp/tls"],
            vec!["cdk-mintd", "get-info"],
        ] {
            assert!(
                CLIArgs::try_parse_from(args).is_err(),
                "rpc client options must not be accepted by cdk-mintd"
            );
        }
    }

    #[test]
    fn parses_remaining_config_commands() {
        for command in ["validate", "export"] {
            CLIArgs::try_parse_from(["cdk-mintd", "config", command, "--file", "/tmp/config"])
                .expect("file command should parse");
        }

        for command in ["show", "discard-pending"] {
            CLIArgs::try_parse_from(["cdk-mintd", "config", command])
                .expect("command should parse");
        }
    }

    #[test]
    fn config_apply_has_no_revision_or_force_flags() {
        for unsupported_flag in ["--expected-revision", "--force"] {
            let result = CLIArgs::try_parse_from([
                "cdk-mintd",
                "config",
                "apply",
                "--file",
                "/tmp/config.toml",
                unsupported_flag,
            ]);

            assert!(result.is_err());
        }
    }
}
