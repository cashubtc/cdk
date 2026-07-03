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
    /// Inspect federation config material without printing member secrets.
    Inspect(FederationInspectArgs),
    /// Run a local fake-wallet operation smoke through the federation wallet connector.
    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    SmokeOperations(FederationSmokeOperationsArgs),
    /// Generate local trusted-dealer federation configs for fake-wallet testing.
    TrustedDealer(FederationTrustedDealerArgs),
    /// Query signed local status from federation members.
    Status(FederationStatusArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationInspectArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Optional private member config JSON path to validate against the public config.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
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

#[cfg(all(feature = "federation", feature = "fakewallet", feature = "sqlite"))]
#[derive(Debug, Clone, Args)]
pub struct FederationSmokeOperationsArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: PathBuf,
    /// Currency unit to use for the operation smoke.
    #[arg(long, default_value = "sat")]
    pub unit: String,
    /// Amount to mint before swap and melt checks.
    #[arg(long, default_value_t = 4)]
    pub mint_amount: u64,
    /// Number of mint quote status attempts before failing.
    #[arg(long, default_value_t = 100)]
    pub quote_retries: u16,
    /// Delay between quote status attempts.
    #[arg(long, default_value_t = 50)]
    pub quote_sleep_ms: u64,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationStatusArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign status requests.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Member ID to query. Repeat to query multiple members. Defaults to every member.
    #[arg(long = "member")]
    pub members: Vec<u16>,
    /// Fail if any queried member reports a non-ready sync state.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_ready: bool,
    /// Fail if queried members disagree on consensus height or latest order digest.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_converged: bool,
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
        let FederationCommand::TrustedDealer(trusted_dealer) = federation.command else {
            panic!("expected federation trusted-dealer command");
        };
        assert_eq!(trusted_dealer.members, 5);
        assert_eq!(trusted_dealer.threshold, 3);
        assert_eq!(trusted_dealer.amounts, vec![1, 2, 4]);
    }

    #[test]
    fn parses_federation_inspect_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "inspect",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
        ])
        .expect("inspect command parses");

        let Some(CliCommand::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::Inspect(inspect) = federation.command else {
            panic!("expected federation inspect command");
        };
        assert_eq!(
            inspect.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            inspect.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[test]
    fn parses_federation_smoke_operations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "smoke-operations",
            "--public-config",
            "federation-public.json",
            "--mint-amount",
            "8",
        ])
        .expect("smoke operations command parses");

        let Some(CliCommand::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(smoke.public_config, PathBuf::from("federation-public.json"));
        assert_eq!(smoke.mint_amount, 8);
    }

    #[test]
    fn parses_federation_status_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "status",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--member",
            "1",
            "--member",
            "3",
            "--require-ready",
            "--require-converged",
        ])
        .expect("status command parses");

        let Some(CliCommand::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::Status(status) = federation.command else {
            panic!("expected federation status command");
        };
        assert_eq!(
            status.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            status.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(status.members, vec![1, 3]);
        assert!(status.require_ready);
        assert!(status.require_converged);
    }
}
