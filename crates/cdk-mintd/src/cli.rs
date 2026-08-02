use std::path::PathBuf;

use clap::Parser;
#[cfg(feature = "iroh")]
use clap::{Args, Subcommand};

#[derive(Debug, Parser)]
#[command(about = "A cashu mint written in rust", author = env!("CARGO_PKG_AUTHORS"), version = env!("CARGO_PKG_VERSION"))]
pub struct CLIArgs {
    #[cfg(feature = "iroh")]
    #[command(subcommand)]
    pub command: Option<CliCommand>,
    #[arg(
        short,
        long,
        help = "Use the <directory> as the location of the database",
        required = false
    )]
    pub work_dir: Option<PathBuf>,
    #[cfg(feature = "sqlcipher")]
    #[arg(short, long, help = "Database password for sqlcipher", required = true)]
    pub password: String,
    #[arg(
        short,
        long,
        help = "Use the <file name> as the location of the config file",
        required = false
    )]
    pub config: Option<PathBuf>,
    #[arg(
        long,
        help = "Read the mint and active payment backend seed phrase from the specified file",
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
}

#[cfg(feature = "iroh")]
#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Initialize or inspect the mint's persistent Iroh endpoint identity.
    Iroh(IrohArgs),
}

#[cfg(feature = "iroh")]
#[derive(Debug, Args)]
pub struct IrohArgs {
    #[command(subcommand)]
    pub command: IrohCommand,
}

#[cfg(feature = "iroh")]
#[derive(Debug, Subcommand)]
pub enum IrohCommand {
    /// Create or load the protected endpoint key and print its stable public URL.
    Init(IrohInitArgs),
}

#[cfg(feature = "iroh")]
#[derive(Debug, Clone, Args)]
pub struct IrohInitArgs {
    /// Protected endpoint-key path; defaults below the mint work directory.
    #[arg(long)]
    pub secret_key_file: Option<PathBuf>,
}

#[cfg(all(test, feature = "iroh"))]
mod iroh_tests {
    use super::*;

    #[test]
    fn parses_iroh_identity_initialization_without_secret_material_in_argv() {
        let args =
            CLIArgs::try_parse_from(["cdk-mintd", "--work-dir", "mint-state", "iroh", "init"])
                .expect("Iroh init command parses");
        let Some(CliCommand::Iroh(iroh)) = args.command else {
            panic!("expected Iroh command");
        };
        assert!(matches!(
            iroh.command,
            IrohCommand::Init(IrohInitArgs {
                secret_key_file: None
            })
        ));
    }
}
