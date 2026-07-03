use std::path::PathBuf;

use clap::Parser;
#[cfg(feature = "federation")]
use clap::{Args, Subcommand};

#[derive(Debug, Parser)]
#[command(about = "A cashu mint written in rust", author = env!("CARGO_PKG_AUTHORS"), version = env!("CARGO_PKG_VERSION"))]
pub struct CLIArgs {
    #[cfg(feature = "federation")]
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

#[cfg(feature = "federation")]
#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Generate or inspect federation setup material.
    Federation(FederationArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Args)]
pub struct FederationArgs {
    #[command(subcommand)]
    pub command: FederationCommand,
}

#[cfg(feature = "federation")]
#[derive(Debug, Subcommand)]
pub enum FederationCommand {
    /// Generate local trusted-dealer federation configs for fake-wallet testing.
    TrustedDealer(FederationTrustedDealerArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationTrustedDealerArgs {
    /// Directory that will receive the generated public config and member folders.
    #[arg(long)]
    pub output_dir: PathBuf,
    /// Number of federation members to generate.
    #[arg(long, default_value_t = 3)]
    pub members: u16,
    /// BLS signature threshold.
    #[arg(long, default_value_t = 2)]
    pub threshold: u16,
    /// Currency unit for the generated aggregate keyset.
    #[arg(long, default_value = "sat")]
    pub unit: String,
    /// Comma-separated denominations to include in the aggregate keyset.
    #[arg(long, value_delimiter = ',', default_value = "1,2,4,8,16,32,64,128")]
    pub amounts: Vec<u64>,
    /// Input fee in parts per thousand for the aggregate keyset.
    #[arg(long, default_value_t = 0)]
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp for the aggregate keyset.
    #[arg(long)]
    pub final_expiry: Option<u64>,
    /// Host used in generated member URLs and listen addresses.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// First member port. Later members increment this port by member ID order.
    #[arg(long, default_value_t = 8085)]
    pub port_start: u16,
    /// Overwrite files previously generated at the target paths.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub force: bool,
}

#[cfg(all(test, feature = "federation"))]
mod tests {
    use super::*;

    #[test]
    fn parses_federation_trusted_dealer_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "trusted-dealer",
            "--output-dir",
            "fed-local",
            "--members",
            "5",
            "--threshold",
            "3",
            "--amounts",
            "1,2,4",
        ])
        .expect("trusted-dealer command parses");

        let Some(CliCommand::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::TrustedDealer(trusted_dealer) = federation.command;
        assert_eq!(trusted_dealer.members, 5);
        assert_eq!(trusted_dealer.threshold, 3);
        assert_eq!(trusted_dealer.amounts, vec![1, 2, 4]);
    }
}
