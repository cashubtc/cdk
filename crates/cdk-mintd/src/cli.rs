use std::path::PathBuf;

#[cfg(all(feature = "federation", any(test, feature = "federation-test-utils")))]
use cdk_common::federation::{
    DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS, DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS,
};
#[cfg(feature = "federation")]
use clap::ValueEnum;
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
    /// Generate or inspect federation setup material.
    #[cfg(feature = "federation")]
    Federation(Box<FederationArgs>),
    /// Manage the database-backed mintd configuration.
    Config(ConfigArgs),
    /// Initialize or inspect the mint Iroh endpoint identity.
    #[cfg(feature = "iroh")]
    Iroh(IrohArgs),
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
    Init(InitConfigArgs),
    /// Validate a TOML document without changing the database.
    Validate(ConfigFileArgs),
    /// Replace the configuration used by the next mintd start.
    Apply(ApplyConfigArgs),
    /// Restore the last configuration known to have been applied.
    Rollback,
    /// Print the stored configuration document.
    Show,
    /// Export the stored configuration document.
    Export(ExportConfigArgs),
}

/// Arguments for initializing database-backed configuration.
#[derive(Debug, Args)]
pub struct InitConfigArgs {
    /// TOML document to import.
    #[arg(long)]
    pub file: PathBuf,
    /// Initialize a database that has never served a mint.
    #[arg(
        long,
        required_unless_present = "existing_mint",
        conflicts_with = "existing_mint"
    )]
    pub new_mint: bool,
    /// Import configuration into a database containing an existing mint.
    #[arg(
        long,
        required_unless_present = "new_mint",
        conflicts_with = "new_mint"
    )]
    pub existing_mint: bool,
    /// Permit an existing mint to create a BDK wallet when no wallet database exists.
    #[arg(long, requires = "existing_mint", conflicts_with = "new_mint")]
    pub allow_new_bdk_wallet: bool,
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
    /// Permit creation of a BDK wallet when no wallet database exists.
    #[arg(long)]
    pub allow_new_bdk_wallet: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_configuration_commands() {
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--new-mint",
            "--file",
            "/tmp/mint.toml",
        ])
        .expect("new-mint initialization should parse");
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--existing-mint",
            "--file",
            "/tmp/mint.toml",
        ])
        .expect("existing-mint initialization should parse");
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "validate",
            "--file",
            "/tmp/mint.toml",
        ])
        .expect("configuration validation should parse");

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
        CLIArgs::try_parse_from(["cdk-mintd", "config", "rollback"])
            .expect("configuration rollback should parse");

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
    fn initialization_requires_exactly_one_mint_mode() {
        assert!(CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--file",
            "/tmp/mint.toml",
        ])
        .is_err());
        assert!(CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--new-mint",
            "--existing-mint",
            "--file",
            "/tmp/mint.toml",
        ])
        .is_err());
        assert!(CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--new-mint",
            "--allow-new-bdk-wallet",
            "--file",
            "/tmp/mint.toml",
        ])
        .is_err());

        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "config",
            "init",
            "--existing-mint",
            "--allow-new-bdk-wallet",
            "--file",
            "/tmp/mint.toml",
        ])
        .expect("existing mint may explicitly allow a new BDK wallet");
        assert!(matches!(
            args.command,
            Some(Commands::Config(ConfigArgs {
                command: ConfigCommands::Init(InitConfigArgs {
                    allow_new_bdk_wallet: true,
                    ..
                }),
            }))
        ));
    }

    #[test]
    fn no_subcommand_still_parses_daemon_startup() {
        let args = CLIArgs::try_parse_from(["cdk-mintd"]).expect("daemon arguments should parse");
        assert!(args.command.is_none());
    }
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
    /// Export a current standard endpoint ticket for a protected identity.
    Ticket(IrohTicketArgs),
}

#[cfg(feature = "iroh")]
#[derive(Debug, Clone, Args)]
pub struct IrohInitArgs {
    /// Protected endpoint-key path; defaults below the mint work directory.
    #[arg(long)]
    pub secret_key_file: Option<PathBuf>,
}

#[cfg(feature = "iroh")]
#[derive(Debug, Clone, Args)]
pub struct IrohTicketArgs {
    /// Protected endpoint-key path; defaults below the mint work directory.
    #[arg(long)]
    pub secret_key_file: Option<PathBuf>,
    /// Direct UDP address to include in the exported endpoint ticket.
    #[arg(long)]
    pub bind_addr: std::net::SocketAddr,
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
    /// Explicitly audit one member's full state against its trusted checkpoint.
    AuditCheckpoint(FederationCheckpointAuditArgs),
    /// Bind a quiesced database backup to its checkpoint and member secrets.
    BackupManifest(FederationBackupManifestArgs),
    /// Compare signed order and exact current state across quiesced federation members.
    CompareDigests(FederationCompareDigestsArgs),
    /// Inspect collected DKG completion records without printing secret shares.
    DkgStatus(FederationDkgStatusArgs),
    /// Inspect federation config material without printing member secrets.
    Inspect(FederationInspectArgs),
    /// Create or join a restartable multi-operator federation setup.
    Setup(FederationSetupArgs),
    /// Generate a fake BOLT11 invoice for local fakewallet smokes.
    #[cfg(feature = "fakewallet")]
    FakeInvoice(FederationFakeInvoiceArgs),
    /// List redacted payment-observation reports from signed member status.
    PaymentObservations(FederationPaymentObservationsArgs),
    /// Run a local fake-wallet operation smoke through the federation wallet connector.
    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    SmokeOperations(FederationSmokeOperationsArgs),
    /// Generate test-only trusted-dealer federation material for local regtest.
    #[cfg(feature = "federation-test-utils")]
    TestTrustedDealer(FederationTrustedDealerArgs),
    /// Query signed local status from federation members.
    Status(FederationStatusArgs),
    /// Query redacted federated on-chain wallet status.
    #[cfg(feature = "federation-bdk")]
    WalletStatus(FederationStatusArgs),
    /// Query one accepted federated on-chain transaction without transaction contents.
    #[cfg(feature = "federation-bdk")]
    TransactionStatus(FederationWalletTransactionArgs),
    /// Immediately retry one exact durably signed transaction.
    #[cfg(feature = "federation-bdk")]
    Rebroadcast(FederationWalletRebroadcastArgs),
    /// Replay accepted Bark state into one member's local wallet.
    #[cfg(feature = "federation-bark")]
    BarkReconcile(FederationBarkReconcileArgs),
    /// Allocate an idempotent Bark-owned CPFP fee-reserve address.
    #[cfg(feature = "federation-bark")]
    BarkRecoveryAddress(FederationBarkRecoveryAddressArgs),
    /// Retry exactly one accepted Bark action.
    #[cfg(feature = "federation-bark")]
    BarkRetry(FederationBarkRetryArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Args)]
pub struct FederationSetupArgs {
    #[command(subcommand)]
    pub command: FederationSetupCommand,
}

#[cfg(feature = "federation")]
#[derive(Debug, Subcommand)]
pub enum FederationSetupCommand {
    /// Create a setup and protected invitations for every joining member.
    Create(FederationSetupCreateArgs),
    /// Issue or explicitly replace one protected invitation.
    Invite(FederationSetupInviteArgs),
    /// Join a setup using a protected invitation file.
    Join(FederationSetupJoinArgs),
    /// Freeze the complete enrollment and policy set into one proposal.
    Propose(FederationSetupConfigArgs),
    /// Review the exact proposal that this member may approve.
    Review(FederationSetupReviewArgs),
    /// Approve exactly one previously reviewed proposal hash.
    Approve(FederationSetupApproveArgs),
    /// Run only the bounded setup coordinator HTTP service.
    Serve(FederationSetupServeArgs),
    /// Show authenticated setup progress and persist a ready manifest.
    Status(FederationSetupStatusArgs),
    /// Validate, publish, and explicitly activate this member's final config.
    Activate(FederationSetupActivateArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupCreateArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Directory for protected local identity and invitation files.
    #[arg(long)]
    pub state_dir: PathBuf,
    /// SQLite database path for restartable setup state.
    #[arg(long)]
    pub database: PathBuf,
    /// Number of fixed federation members.
    #[arg(long)]
    pub members: u16,
    /// BLS signature threshold.
    #[arg(long)]
    pub threshold: u16,
    /// Bitcoin network committed by the setup proposal.
    #[arg(long, value_enum)]
    pub network: FederationSetupNetworkArg,
    /// Creator's wallet-facing Cashu URL.
    #[arg(long)]
    pub public_mint_url: String,
    /// Creator's private federation API URL.
    #[arg(long)]
    pub federation_api_url: String,
    /// Setup-only coordinator URL embedded in invitations.
    #[arg(long)]
    pub coordinator_url: String,
    /// Matching member observations required for payment state.
    #[arg(long)]
    pub payment_observation_threshold: Option<u16>,
    /// Accepted journal operations between scheduled checkpoints.
    #[arg(
        long,
        default_value_t = cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS
    )]
    pub checkpoint_interval_journal_operations: u64,
    /// Finalized consensus items between scheduled checkpoints.
    #[arg(
        long,
        default_value_t = cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS
    )]
    pub checkpoint_interval_finalized_items: u64,
    /// Canonical finalized-history bytes between scheduled checkpoints.
    #[arg(
        long,
        default_value_t = cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_CANONICAL_BYTES
    )]
    pub checkpoint_interval_canonical_bytes: u64,
    /// Invitation validity in seconds.
    #[arg(long, default_value_t = 86_400)]
    pub invitation_lifetime_seconds: u64,
    /// Currency unit for the first BLS keyset.
    #[arg(long, default_value = "sat")]
    pub unit: String,
    /// Comma-separated denominations for the first BLS keyset.
    #[arg(long, value_delimiter = ',', default_value = "1,2,4,8,16,32,64,128")]
    pub amounts: Vec<u64>,
    /// Input fee in parts per thousand for the first BLS keyset.
    #[arg(long, default_value_t = 0)]
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp for the first BLS keyset.
    #[arg(long)]
    pub final_expiry: Option<u64>,
    /// Initial reusable FROST root epoch.
    #[arg(long, default_value_t = 1)]
    pub frost_root_key_epoch: u64,
    /// Initial CDK on-chain application-key epoch.
    #[arg(long, default_value_t = 1)]
    pub frost_application_key_epoch: u64,
    /// Required confirmations for federated on-chain transactions.
    #[arg(long, default_value_t = 1)]
    pub frost_minimum_confirmations: u32,
    /// Consensus transaction fee-policy version.
    #[arg(long, default_value_t = 1)]
    pub frost_fee_policy_version: u16,
    /// Compressed public key of the accepted Ark server for an optional Bark application.
    #[arg(long, requires_all = ["frost_bark_application_key_epoch", "frost_bark_policy_config"])]
    pub frost_bark_ark_server_public_key: Option<String>,
    /// Initial Bark application-key epoch derived from the reusable FROST root.
    #[arg(long, requires_all = ["frost_bark_ark_server_public_key", "frost_bark_policy_config"])]
    pub frost_bark_application_key_epoch: Option<u64>,
    /// JSON file containing the exact accepted Bark fee, CLTV, and recovery policy.
    #[arg(long, requires_all = ["frost_bark_ark_server_public_key", "frost_bark_application_key_epoch"])]
    pub frost_bark_policy_config: Option<PathBuf>,
    /// Permit plain HTTP only when every configured URL is localhost.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub allow_insecure_localhost: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupInviteArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Member slot receiving the invitation.
    #[arg(long)]
    pub member: u16,
    /// Protected destination for the invitation credential.
    #[arg(long)]
    pub output: PathBuf,
    /// Expire an existing unconsumed invitation for this slot.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub replace: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupJoinArgs {
    /// Protected invitation credential file received from the creator.
    #[arg(long)]
    pub invitation_file: PathBuf,
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Directory for this operator's protected local identity.
    #[arg(long)]
    pub state_dir: PathBuf,
    /// SQLite database path for restartable setup state.
    #[arg(long)]
    pub database: PathBuf,
    /// This member's wallet-facing Cashu URL.
    #[arg(long)]
    pub public_mint_url: String,
    /// This member's private federation API URL.
    #[arg(long)]
    pub federation_api_url: String,
    /// Permit plain HTTP only when every configured URL is localhost.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub allow_insecure_localhost: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupConfigArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupReviewArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Print the bounded stable machine-readable review document.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub json: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupApproveArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Exact proposal hash displayed by `setup review`.
    #[arg(long)]
    pub proposal_hash: String,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupServeArgs {
    /// Protected creator setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Socket address for the isolated setup-only HTTP service.
    #[arg(long)]
    pub listen: std::net::SocketAddr,
    /// Maximum durable plan-and-deliver ticks for a certified DKG invocation.
    #[arg(long, default_value_t = 1_200)]
    pub dkg_max_ticks: usize,
    /// Delay between incomplete certified DKG ticks in milliseconds.
    #[arg(long, default_value_t = 250)]
    pub dkg_retry_delay_ms: u64,
    /// Overall certified DKG driver budget in seconds.
    #[arg(long, default_value_t = 600)]
    pub dkg_ceremony_timeout_seconds: u64,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupStatusArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
    /// Print the stable machine-readable status document.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub json: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationSetupActivateArgs {
    /// Protected local setup config path.
    #[arg(long)]
    pub config: PathBuf,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationSetupNetworkArg {
    /// Bitcoin mainnet.
    Bitcoin,
    /// Bitcoin testnet.
    Testnet,
    /// Bitcoin signet.
    Signet,
    /// Bitcoin regtest.
    Regtest,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationBackupManifestArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path for the backed-up database.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Destination for the checkpoint-bound backup manifest.
    #[arg(long)]
    pub output: PathBuf,
    /// Overwrite an existing manifest file.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub force: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationCheckpointAuditArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the audit request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Member ID whose local database should be audited.
    #[arg(long)]
    pub member: u16,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationDkgStatusArgs {
    /// Shared setup public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to identify the local member.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Read persisted DKG completion records from the configured mint database.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub from_database: bool,
    /// Report redacted durable record counts without cryptographic finalization checks.
    #[arg(
        long,
        action = clap::ArgAction::SetTrue,
        requires = "from_database",
        conflicts_with_all = ["require_ready", "final_config_dir"]
    )]
    pub progress_only: bool,
    /// Completed local DKG keyset-share JSON record. Repeat for multiple keysets.
    #[arg(long = "keyset-share")]
    pub keyset_shares: Vec<PathBuf>,
    /// DKG transcript-signature JSON record. Repeat for every collected member signature.
    #[arg(long = "transcript-signature")]
    pub transcript_signatures: Vec<PathBuf>,
    /// DKG finalized-config activation JSON record. Repeat for every member confirmation.
    #[arg(long = "activation")]
    pub activations: Vec<PathBuf>,
    /// Fail if final DKG config material cannot be assembled from the supplied records.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_ready: bool,
    /// Write finalized public and local-private configs into this directory when ready.
    #[arg(long)]
    pub final_config_dir: Option<PathBuf>,
    /// Overwrite existing files in --final-config-dir.
    #[arg(long, action = clap::ArgAction::SetTrue, requires = "final_config_dir")]
    pub force: bool,
    /// Explicit Bitcoin network for bootstrapping the first CDK FROST application.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_cdk_network: Option<String>,
    /// Consensus confirmation depth for the bootstrapped CDK FROST application.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_cdk_minimum_confirmations: Option<u32>,
    /// Consensus transaction-policy version for the bootstrapped CDK FROST application.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_cdk_fee_policy_version: Option<u16>,
    /// Initial application key epoch derived from the application-neutral FROST root.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_cdk_application_key_epoch: Option<u64>,
    /// Explicit Bitcoin network for the initial federated Bark application.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_network: Option<String>,
    /// Compressed public key of the single accepted Ark server.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_ark_server_public_key: Option<String>,
    /// Initial Bark application key epoch derived from the reusable FROST root.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_application_key_epoch: Option<u64>,
    /// JSON file containing the exact accepted Bark fee, CLTV, and recovery policy.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_policy_config: Option<PathBuf>,
    /// Required confirmations for Bark recovery transactions.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_recovery_minimum_confirmations: Option<u32>,
    /// Consensus transaction-policy version for Bark recovery transactions.
    #[arg(long, requires_all = ["from_database", "final_config_dir"])]
    pub frost_bark_recovery_fee_policy_version: Option<u16>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationCompareDigestsArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign status requests.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Member ID to query. Repeat to query multiple members. Defaults to every member.
    #[arg(long = "member")]
    pub members: Vec<u16>,
    /// Print digest groups without failing when members are not converged.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub allow_divergence: bool,
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
pub struct FederationPaymentObservationsArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign status requests.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Optional payment-observation context filter.
    #[arg(long, value_enum)]
    pub context: Option<FederationPaymentObservationContextArg>,
    /// Optional quote ID filter.
    #[arg(long)]
    pub quote: Option<String>,
    /// Member ID to query. Repeat to query multiple members. Defaults to every member.
    #[arg(long = "member")]
    pub members: Vec<u16>,
    /// Include resolved quorum reports; by default only unresolved or conflicting reports are shown.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub include_resolved: bool,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, Args)]
pub struct FederationFakeInvoiceArgs {
    /// Invoice amount in millisatoshis.
    #[arg(long)]
    pub amount_msat: u64,
    /// Plain invoice description for the default paid fakewallet path.
    #[arg(long, default_value = "federation cli smoke melt")]
    pub description: String,
    /// Fakewallet payment state encoded into the invoice description.
    #[arg(long, value_enum, default_value_t = FederationFakeInvoiceStateArg::Paid)]
    pub state: FederationFakeInvoiceStateArg,
    /// Make the fakewallet pay_invoice call return an error.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub pay_error: bool,
    /// Make the fakewallet check_outgoing_payment call return an error.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub check_error: bool,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationFakeInvoiceStateArg {
    /// Fakewallet reports the outgoing payment as paid.
    Paid,
    /// Fakewallet reports the outgoing payment as pending.
    Pending,
    /// Fakewallet reports the outgoing payment as unpaid.
    Unpaid,
    /// Fakewallet reports the outgoing payment as failed.
    Failed,
    /// Fakewallet reports the outgoing payment as unknown.
    Unknown,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationPaymentObservationContextArg {
    /// Incoming payment observation for a mint quote.
    MintQuote,
    /// Outgoing payment observation for a melt quote.
    MeltQuote,
}

#[cfg(all(feature = "federation", any(test, feature = "federation-test-utils")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationTrustedDealerDatabaseEngine {
    /// Generate member configs backed by per-member SQLite files.
    Sqlite,
    /// Generate member configs backed by PostgreSQL storage.
    Postgres,
}

#[cfg(all(feature = "federation", any(test, feature = "federation-test-utils")))]
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
    /// Accepted-operation interval between scheduled checkpoint candidates.
    #[arg(
        long,
        default_value_t = DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS
    )]
    pub checkpoint_interval_journal_operations: u64,
    /// Finalized-item interval between scheduled checkpoint candidates.
    #[arg(long, default_value_t = DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS)]
    pub checkpoint_interval_finalized_items: u64,
    /// Host used in generated member URLs and listen addresses.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// First member port. Later members increment this port by member ID order.
    #[arg(long, default_value_t = 8085)]
    pub port_start: u16,
    /// Mint database engine for generated member configs.
    #[arg(long, value_enum, default_value = "sqlite")]
    pub database_engine: FederationTrustedDealerDatabaseEngine,
    /// PostgreSQL URL template used when --database-engine postgres. Must contain {member}.
    #[arg(long)]
    pub postgres_url_template: Option<String>,
    /// PostgreSQL base URL used when --database-engine postgres. Generates one schema per member.
    #[arg(long)]
    pub postgres_base_url: Option<String>,
    /// Also generate local fakewallet member TOMLs and a smoke script for development/regtest.
    #[cfg(feature = "fakewallet")]
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub local_fakewallet_configs: bool,
    /// Deterministic test seed for reproducible local federation material. Do not use for production.
    #[arg(long)]
    pub deterministic_test_seed: Option<String>,
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
    /// Private member config JSON path for scenarios that submit private federation operations.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Currency unit to use for the operation smoke.
    #[arg(long, default_value = "sat")]
    pub unit: String,
    /// Amount to mint before swap and melt checks.
    #[arg(long, default_value_t = 4)]
    pub mint_amount: u64,
    /// Smoke scenario to run.
    #[arg(long, value_enum, default_value_t = FederationSmokeScenario::Standard)]
    pub scenario: FederationSmokeScenario,
    /// Wallet SQLite database path for resumable smoke scenarios.
    #[arg(long)]
    pub wallet_db: Option<PathBuf>,
    /// Deterministic wallet seed offset for repeatable operation smoke batches.
    #[arg(long, default_value_t = 0)]
    pub seed_offset: u8,
    /// Number of standard/offline operation smoke batches to run.
    #[arg(long, default_value_t = 1)]
    pub operation_batches: u8,
    /// Number of mint quote status attempts before failing.
    #[arg(long, default_value_t = 100)]
    pub quote_retries: u16,
    /// Delay between quote status attempts.
    #[arg(long, default_value_t = 50)]
    pub quote_sleep_ms: u64,
    /// Maximum seconds allowed for one smoke scenario before failing.
    #[arg(long)]
    pub operation_timeout_seconds: Option<u64>,
}

#[cfg(all(feature = "federation", feature = "fakewallet", feature = "sqlite"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationSmokeScenario {
    /// Run the ordinary mint, swap, melt, restore smoke.
    Standard,
    /// Run ordinary wallet operations while one non-primary member is offline.
    OfflineMemberOperations,
    /// Mint funds and leave a melt pending in the wallet database.
    PendingMeltStart,
    /// Resume and finalize the pending melt stored in the wallet database.
    PendingMeltFinish,
    /// Mint funds and leave a melt pending that later resolves as failed.
    PendingMeltFailStart,
    /// Resume and compensate the failed pending melt stored in the wallet database.
    PendingMeltFailFinish,
    /// Rotate to the inactive aggregate keyset and run wallet operations against the rotated view.
    KeysetRotation,
    /// Rotate to the inactive aggregate keyset while one non-primary member is offline.
    OfflineMemberKeysetRotation,
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
    /// Fail if queried members disagree on height, order digest, or materialized-state digest.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_converged: bool,
    /// Fail if any queried member is more than this many accepted operations behind the highest queried member.
    #[arg(long)]
    pub max_operation_lag: Option<u64>,
    /// Fail if queried members with accepted operations lack a trusted checkpoint quorum.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_checkpoint_quorum: bool,
    /// Fail if queried members report or reveal equal-height digest mismatches.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_digest_alerts: bool,
    /// Fail if queried members report stale or conflicting payment observations.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_payment_observation_alerts: bool,
    /// Fail if queried members report invalid-operation alert buckets.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_invalid_operation_alerts: bool,
}

#[cfg(feature = "federation-bdk")]
#[derive(Debug, Clone, Args)]
pub struct FederationWalletTransactionArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the operator request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Canonical transaction proposal ID.
    #[arg(long)]
    pub proposal_id: String,
    /// Member to query; defaults to the signing member.
    #[arg(long)]
    pub member: Option<u16>,
}

#[cfg(feature = "federation-bdk")]
#[derive(Debug, Clone, Args)]
pub struct FederationWalletRebroadcastArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the operator request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Canonical transaction proposal ID.
    #[arg(long)]
    pub proposal_id: String,
    /// Member whose local chain backend should retry; defaults to the signing member.
    #[arg(long)]
    pub member: Option<u16>,
    /// Acknowledge that only the exact persisted bytes are retried and no policy is bypassed.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub acknowledge_exact_byte_retry: bool,
}

#[cfg(all(feature = "federation", feature = "federation-bark"))]
#[derive(Debug, Clone, Args)]
pub struct FederationBarkReconcileArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the operator request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Member whose local Bark projection should be reconciled; defaults to the signing member.
    #[arg(long)]
    pub member: Option<u16>,
}

#[cfg(all(feature = "federation", feature = "federation-bark"))]
#[derive(Debug, Clone, Args)]
pub struct FederationBarkRecoveryAddressArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the operator request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Stable 1-64 character idempotency key for this reserve address.
    #[arg(long)]
    pub reserve_id: String,
    /// Member that should allocate the address; defaults to the signing member.
    #[arg(long)]
    pub member: Option<u16>,
}

#[cfg(all(feature = "federation", feature = "federation-bark"))]
#[derive(Debug, Clone, Args)]
pub struct FederationBarkRetryArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign the operator request.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Exact accepted Bark action identity.
    #[arg(long)]
    pub action_id: String,
    /// Member whose local worker should retry; defaults to the signing member.
    #[arg(long)]
    pub member: Option<u16>,
    /// Acknowledge that the exact accepted action may repeat idempotent Ark effects.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub acknowledge_bark_side_effects: bool,
}

#[cfg(all(test, feature = "federation"))]
mod federation_tests {
    use super::*;

    #[test]
    fn parses_multi_operator_setup_commands_without_invitation_in_argv() {
        let create = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "create",
            "--config",
            "operator/setup.json",
            "--state-dir",
            "operator",
            "--database",
            "operator/mint.sqlite",
            "--members",
            "5",
            "--threshold",
            "3",
            "--network",
            "bitcoin",
            "--public-mint-url",
            "https://mint.example",
            "--federation-api-url",
            "https://peer.example",
            "--coordinator-url",
            "https://setup.example",
        ])
        .expect("setup create command parses");
        let Some(Commands::Federation(create)) = create.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            create.command,
            FederationCommand::Setup(FederationSetupArgs {
                command: FederationSetupCommand::Create(FederationSetupCreateArgs {
                    members: 5,
                    threshold: 3,
                    ..
                })
            })
        ));

        let join = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "join",
            "--invitation-file",
            "operator/member.invite",
            "--config",
            "operator/setup.json",
            "--state-dir",
            "operator",
            "--database",
            "operator/mint.sqlite",
            "--public-mint-url",
            "https://mint.example",
            "--federation-api-url",
            "https://peer.example",
        ])
        .expect("setup join command parses");
        let Some(Commands::Federation(join)) = join.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            join.command,
            FederationCommand::Setup(FederationSetupArgs {
                command: FederationSetupCommand::Join(_)
            })
        ));

        assert!(CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "join",
            "cashufedsetupAsecret-must-not-be-an-argument",
        ])
        .is_err());

        for command in ["propose", "review"] {
            CLIArgs::try_parse_from([
                "cdk-mintd",
                "federation",
                "setup",
                command,
                "--config",
                "operator/setup.json",
            ])
            .expect("setup config command parses");
        }
        let serve = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "serve",
            "--config",
            "operator/setup.json",
            "--listen",
            "127.0.0.1:18080",
            "--dkg-max-ticks",
            "2400",
            "--dkg-retry-delay-ms",
            "500",
            "--dkg-ceremony-timeout-seconds",
            "1200",
        ])
        .expect("setup-only serve command parses");
        let Some(Commands::Federation(serve)) = serve.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            serve.command,
            FederationCommand::Setup(FederationSetupArgs {
                command: FederationSetupCommand::Serve(FederationSetupServeArgs {
                    dkg_max_ticks: 2400,
                    dkg_retry_delay_ms: 500,
                    dkg_ceremony_timeout_seconds: 1200,
                    ..
                })
            })
        ));
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "status",
            "--config",
            "operator/setup.json",
            "--json",
        ])
        .expect("setup status command parses");
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "activate",
            "--config",
            "operator/setup.json",
        ])
        .expect("local setup activation command parses");
        CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "setup",
            "approve",
            "--config",
            "operator/setup.json",
            "--proposal-hash",
            &"11".repeat(32),
        ])
        .expect("setup approval command parses");
    }

    #[test]
    fn production_cli_rejects_removed_file_mediated_dkg_setup_commands() {
        for command in ["dkg-member", "dkg-roster", "dkg-bind"] {
            let error = CLIArgs::try_parse_from(["cdk-mintd", "federation", command])
                .expect_err("removed DKG setup command must not parse");
            assert!(error.to_string().contains("unrecognized subcommand"));
        }
    }

    #[cfg(not(feature = "federation-test-utils"))]
    #[test]
    fn production_cli_does_not_expose_trusted_dealer() {
        let err = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "test-trusted-dealer",
            "--output-dir",
            "local-test",
        ])
        .expect_err("test-only command must not parse in a production build");
        assert!(err.to_string().contains("unrecognized subcommand"));
    }

    #[cfg(not(all(feature = "fakewallet", feature = "sqlite")))]
    #[test]
    fn production_cli_does_not_expose_federation_smoke_operations() {
        let err = CLIArgs::try_parse_from(["cdk-mintd", "federation", "smoke-operations"])
            .expect_err("development-only smoke command must not parse in a production build");
        assert!(err.to_string().contains("unrecognized subcommand"));
    }

    #[test]
    fn parses_federation_dkg_status_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-status",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--from-database",
            "--keyset-share",
            "member-1/dkg-keyset-share.json",
            "--transcript-signature",
            "member-1/dkg-transcript-signature.json",
            "--require-ready",
            "--final-config-dir",
            "member-1/finalized",
            "--frost-cdk-network",
            "regtest",
            "--frost-cdk-minimum-confirmations",
            "1",
            "--frost-cdk-fee-policy-version",
            "1",
            "--frost-cdk-application-key-epoch",
            "1",
            "--frost-bark-network",
            "regtest",
            "--frost-bark-ark-server-public-key",
            "03774ae7f858a9411e5ef4246b70c65aac5649980be5c17891bbec17895da008cb",
            "--frost-bark-application-key-epoch",
            "1",
            "--frost-bark-policy-config",
            "bark-policy.json",
            "--force",
        ])
        .expect("dkg status command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::DkgStatus(status) = federation.command else {
            panic!("expected federation dkg-status command");
        };
        assert_eq!(
            status.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            status.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert!(status.from_database);
        assert!(!status.progress_only);
        assert_eq!(
            status.keyset_shares,
            vec![PathBuf::from("member-1/dkg-keyset-share.json")]
        );
        assert_eq!(
            status.transcript_signatures,
            vec![PathBuf::from("member-1/dkg-transcript-signature.json")]
        );
        assert!(status.require_ready);
        assert_eq!(status.frost_bark_network.as_deref(), Some("regtest"));
        assert_eq!(
            status.frost_bark_policy_config,
            Some(PathBuf::from("bark-policy.json"))
        );
        assert_eq!(
            status.final_config_dir,
            Some(PathBuf::from("member-1/finalized"))
        );
        assert!(status.force);
        assert_eq!(status.frost_cdk_network.as_deref(), Some("regtest"));
        assert_eq!(status.frost_cdk_minimum_confirmations, Some(1));
        assert_eq!(status.frost_cdk_fee_policy_version, Some(1));
        assert_eq!(status.frost_cdk_application_key_epoch, Some(1));
    }

    #[test]
    fn parses_federation_dkg_progress_only_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-status",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--from-database",
            "--progress-only",
        ])
        .expect("DKG progress command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::DkgStatus(status) = federation.command else {
            panic!("expected federation dkg-status command");
        };
        assert!(status.from_database);
        assert!(status.progress_only);
        assert!(!status.require_ready);
        assert!(status.final_config_dir.is_none());
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn parses_federation_fake_invoice_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "fake-invoice",
            "--amount-msat",
            "10000",
            "--state",
            "failed",
            "--pay-error",
        ])
        .expect("fake invoice command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::FakeInvoice(fake_invoice) = federation.command else {
            panic!("expected federation fake-invoice command");
        };
        assert_eq!(fake_invoice.amount_msat, 10000);
        assert_eq!(fake_invoice.state, FederationFakeInvoiceStateArg::Failed);
        assert!(fake_invoice.pay_error);
    }

    #[cfg(feature = "federation-test-utils")]
    #[test]
    fn parses_federation_trusted_dealer_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "test-trusted-dealer",
            "--output-dir",
            "fed-local",
            "--members",
            "5",
            "--threshold",
            "3",
            "--amounts",
            "1,2,4",
            "--deterministic-test-seed",
            "fixture-seed",
        ])
        .expect("trusted-dealer command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::TestTrustedDealer(trusted_dealer) = federation.command else {
            panic!("expected federation trusted-dealer command");
        };
        assert_eq!(trusted_dealer.members, 5);
        assert_eq!(trusted_dealer.threshold, 3);
        assert_eq!(trusted_dealer.amounts, vec![1, 2, 4]);
        assert_eq!(
            trusted_dealer.database_engine,
            FederationTrustedDealerDatabaseEngine::Sqlite
        );
        assert_eq!(trusted_dealer.postgres_url_template, None);
        assert_eq!(trusted_dealer.postgres_base_url, None);
        assert_eq!(
            trusted_dealer.deterministic_test_seed.as_deref(),
            Some("fixture-seed")
        );
        #[cfg(feature = "fakewallet")]
        assert!(!trusted_dealer.local_fakewallet_configs);
    }

    #[cfg(feature = "federation-test-utils")]
    #[test]
    fn parses_federation_trusted_dealer_local_fakewallet_opt_in() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "test-trusted-dealer",
            "--output-dir",
            "fed-local",
            "--local-fakewallet-configs",
        ])
        .expect("trusted-dealer local fakewallet command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::TestTrustedDealer(trusted_dealer) = federation.command else {
            panic!("expected federation trusted-dealer command");
        };
        assert!(trusted_dealer.local_fakewallet_configs);
    }

    #[cfg(feature = "federation-test-utils")]
    #[test]
    fn parses_federation_trusted_dealer_postgres_database_options() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "test-trusted-dealer",
            "--output-dir",
            "fed-local",
            "--database-engine",
            "postgres",
            "--postgres-url-template",
            "postgresql://cdk:cdk@localhost:5432/cdk_member_{member}",
        ])
        .expect("trusted-dealer postgres command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::TestTrustedDealer(trusted_dealer) = federation.command else {
            panic!("expected federation trusted-dealer command");
        };
        assert_eq!(
            trusted_dealer.database_engine,
            FederationTrustedDealerDatabaseEngine::Postgres
        );
        assert_eq!(
            trusted_dealer.postgres_url_template,
            Some("postgresql://cdk:cdk@localhost:5432/cdk_member_{member}".to_string())
        );
        assert_eq!(trusted_dealer.postgres_base_url, None);
    }

    #[cfg(feature = "federation-test-utils")]
    #[test]
    fn parses_federation_trusted_dealer_postgres_base_url() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "test-trusted-dealer",
            "--output-dir",
            "fed-local",
            "--database-engine",
            "postgres",
            "--postgres-base-url",
            "postgresql://cdk:cdk@localhost:5432/cdk_mint",
        ])
        .expect("trusted-dealer postgres base-url command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::TestTrustedDealer(trusted_dealer) = federation.command else {
            panic!("expected federation trusted-dealer command");
        };
        assert_eq!(
            trusted_dealer.database_engine,
            FederationTrustedDealerDatabaseEngine::Postgres
        );
        assert_eq!(trusted_dealer.postgres_url_template, None);
        assert_eq!(
            trusted_dealer.postgres_base_url,
            Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string())
        );
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

        let Some(Commands::Federation(federation)) = args.command else {
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

    #[test]
    fn parses_federation_compare_digests_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "compare-digests",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--member",
            "1",
            "--member",
            "3",
            "--allow-divergence",
        ])
        .expect("compare digests command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::CompareDigests(compare) = federation.command else {
            panic!("expected federation compare digests command");
        };
        assert_eq!(
            compare.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            compare.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(compare.members, vec![1, 3]);
        assert!(compare.allow_divergence);
    }

    #[test]
    fn parses_federation_payment_observations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "payment-observations",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--context",
            "melt-quote",
            "--quote",
            "quote-1",
            "--member",
            "1",
            "--member",
            "3",
            "--include-resolved",
        ])
        .expect("payment observations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::PaymentObservations(observations) = federation.command else {
            panic!("expected federation payment observations command");
        };
        assert_eq!(
            observations.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            observations.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(
            observations.context,
            Some(FederationPaymentObservationContextArg::MeltQuote)
        );
        assert_eq!(observations.quote, Some("quote-1".to_string()));
        assert_eq!(observations.members, vec![1, 3]);
        assert!(observations.include_resolved);
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
            "--scenario",
            "pending-melt-fail-start",
            "--wallet-db",
            "wallet.sqlite",
            "--operation-timeout-seconds",
            "42",
        ])
        .expect("smoke operations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(smoke.public_config, PathBuf::from("federation-public.json"));
        assert_eq!(smoke.mint_amount, 8);
        assert_eq!(
            smoke.scenario,
            FederationSmokeScenario::PendingMeltFailStart
        );
        assert_eq!(smoke.wallet_db, Some(PathBuf::from("wallet.sqlite")));
        assert_eq!(smoke.private_config, None);
        assert_eq!(smoke.operation_timeout_seconds, Some(42));
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[test]
    fn parses_federation_keyset_rotation_smoke_operations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "smoke-operations",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--scenario",
            "keyset-rotation",
        ])
        .expect("keyset rotation smoke operations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(
            smoke.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(smoke.scenario, FederationSmokeScenario::KeysetRotation);
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[test]
    fn parses_federation_offline_keyset_rotation_smoke_operations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "smoke-operations",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--scenario",
            "offline-member-keyset-rotation",
        ])
        .expect("offline keyset rotation smoke operations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(
            smoke.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(
            smoke.scenario,
            FederationSmokeScenario::OfflineMemberKeysetRotation
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[test]
    fn parses_federation_offline_smoke_operations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "smoke-operations",
            "--public-config",
            "federation-public.json",
            "--scenario",
            "offline-member-operations",
            "--seed-offset",
            "3",
        ])
        .expect("offline smoke operations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(
            smoke.scenario,
            FederationSmokeScenario::OfflineMemberOperations
        );
        assert_eq!(smoke.seed_offset, 3);
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
            "--max-operation-lag",
            "2",
            "--require-checkpoint-quorum",
            "--require-no-digest-alerts",
            "--require-no-payment-observation-alerts",
            "--require-no-invalid-operation-alerts",
        ])
        .expect("status command parses");

        let Some(Commands::Federation(federation)) = args.command else {
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
        assert_eq!(status.max_operation_lag, Some(2));
        assert!(status.require_checkpoint_quorum);
        assert!(status.require_no_digest_alerts);
        assert!(status.require_no_payment_observation_alerts);
        assert!(status.require_no_invalid_operation_alerts);
    }

    #[test]
    fn parses_federation_checkpoint_audit_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "audit-checkpoint",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--member",
            "2",
        ])
        .expect("checkpoint audit command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::AuditCheckpoint(audit) = federation.command else {
            panic!("expected federation checkpoint audit command");
        };
        assert_eq!(
            audit.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            audit.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(audit.member, 2);
    }

    #[test]
    fn parses_federation_backup_manifest_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "backup-manifest",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--output",
            "member-1/database-backup-manifest.json",
            "--force",
        ])
        .expect("backup manifest command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::BackupManifest(backup) = federation.command else {
            panic!("expected federation backup manifest command");
        };
        assert_eq!(
            backup.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            backup.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(
            backup.output,
            PathBuf::from("member-1/database-backup-manifest.json")
        );
        assert!(backup.force);
    }

    #[cfg(feature = "federation-bdk")]
    #[test]
    fn parses_federated_wallet_operator_commands_and_guards() {
        let proposal_id = "11".repeat(32);
        let transaction = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "transaction-status",
            "--proposal-id",
            &proposal_id,
            "--member",
            "2",
        ])
        .expect("transaction status command parses");
        let Some(Commands::Federation(transaction)) = transaction.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            transaction.command,
            FederationCommand::TransactionStatus(FederationWalletTransactionArgs {
                member: Some(2),
                ..
            })
        ));

        let rebroadcast = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "rebroadcast",
            "--proposal-id",
            &proposal_id,
            "--acknowledge-exact-byte-retry",
        ])
        .expect("rebroadcast command parses");
        let Some(Commands::Federation(rebroadcast)) = rebroadcast.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            rebroadcast.command,
            FederationCommand::Rebroadcast(FederationWalletRebroadcastArgs {
                acknowledge_exact_byte_retry: true,
                ..
            })
        ));
    }

    #[cfg(feature = "federation-bark")]
    #[test]
    fn parses_federated_bark_operator_commands_and_guard() {
        let reconcile =
            CLIArgs::try_parse_from(["cdk-mintd", "federation", "bark-reconcile", "--member", "2"])
                .expect("Bark reconcile command parses");
        let Some(Commands::Federation(reconcile)) = reconcile.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            reconcile.command,
            FederationCommand::BarkReconcile(FederationBarkReconcileArgs {
                member: Some(2),
                ..
            })
        ));

        let recovery_address = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "bark-recovery-address",
            "--reserve-id",
            "operator-batch-7",
            "--member",
            "3",
        ])
        .expect("Bark recovery address command parses");
        let Some(Commands::Federation(recovery_address)) = recovery_address.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            recovery_address.command,
            FederationCommand::BarkRecoveryAddress(FederationBarkRecoveryAddressArgs {
                member: Some(3),
                reserve_id,
                ..
            }) if reserve_id == "operator-batch-7"
        ));

        let retry = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "bark-retry",
            "--action-id",
            &"11".repeat(32),
            "--acknowledge-bark-side-effects",
        ])
        .expect("Bark retry command parses");
        let Some(Commands::Federation(retry)) = retry.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            retry.command,
            FederationCommand::BarkRetry(FederationBarkRetryArgs {
                acknowledge_bark_side_effects: true,
                ..
            })
        ));
    }
}

#[cfg(all(test, feature = "iroh"))]
mod iroh_tests {
    use super::*;

    #[test]
    fn parses_iroh_identity_initialization_without_secret_material_in_argv() {
        let args =
            CLIArgs::try_parse_from(["cdk-mintd", "--work-dir", "mint-state", "iroh", "init"])
                .expect("Iroh init command parses");
        let Some(Commands::Iroh(iroh)) = args.command else {
            panic!("expected Iroh command");
        };
        assert!(matches!(
            iroh.command,
            IrohCommand::Init(IrohInitArgs {
                secret_key_file: None
            })
        ));
    }

    #[test]
    fn parses_current_iroh_endpoint_ticket_export() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "--work-dir",
            "mint-state",
            "iroh",
            "ticket",
            "--bind-addr",
            "127.0.0.1:8092",
        ])
        .expect("Iroh ticket command parses");
        let Some(Commands::Iroh(iroh)) = args.command else {
            panic!("expected Iroh command");
        };
        assert!(matches!(
            iroh.command,
            IrohCommand::Ticket(IrohTicketArgs {
                secret_key_file: None,
                bind_addr,
            }) if bind_addr == "127.0.0.1:8092".parse().expect("test bind address")
        ));
    }
}
