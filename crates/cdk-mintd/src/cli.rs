use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
#[cfg(feature = "federation")]
use clap::ValueEnum;

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
    #[cfg(feature = "federation")]
    /// Generate or inspect federation setup material.
    Federation(FederationArgs),
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

#[cfg(feature = "federation")]
#[derive(Debug, Args)]
pub struct FederationArgs {
    #[command(subcommand)]
    pub command: FederationCommand,
}

#[cfg(feature = "federation")]
#[derive(Debug, Subcommand)]
pub enum FederationCommand {
    /// Compare signed order and materialized-state digests across federation members.
    CompareDigests(FederationCompareDigestsArgs),
    /// Generate one member's independent DKG identity enrollment.
    DkgMember(FederationDkgMemberArgs),
    /// Assemble a setup-only federation roster from public member enrollments.
    DkgRoster(FederationDkgRosterArgs),
    /// Bind one member's private identity to an assembled DKG roster.
    DkgBind(FederationDkgBindArgs),
    /// Inspect collected DKG completion records without printing secret shares.
    DkgStatus(FederationDkgStatusArgs),
    /// Inspect federation config material without printing member secrets.
    Inspect(FederationInspectArgs),
    /// Generate a fake BOLT11 invoice for local fakewallet smokes.
    #[cfg(feature = "fakewallet")]
    FakeInvoice(FederationFakeInvoiceArgs),
    /// List redacted payment-observation reports from signed member status.
    PaymentObservations(FederationPaymentObservationsArgs),
    /// Propose a payment-observation quarantine operation over signed private REST.
    QuarantinePaymentObservation(FederationQuarantinePaymentObservationArgs),
    /// Run a local fake-wallet operation smoke through the federation wallet connector.
    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    SmokeOperations(FederationSmokeOperationsArgs),
    /// Generate test-only trusted-dealer federation material for local regtest.
    #[cfg(feature = "federation-test-utils")]
    TestTrustedDealer(FederationTrustedDealerArgs),
    /// Query signed local status from federation members.
    Status(FederationStatusArgs),
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationDkgMemberArgs {
    /// Stable non-zero member ID.
    #[arg(long)]
    pub member_id: u16,
    /// Public Cashu mint URL exposed to wallets.
    #[arg(long)]
    pub public_mint_url: String,
    /// Private federation REST URL exposed to peers.
    #[arg(long)]
    pub federation_api_url: String,
    /// Destination for the public member enrollment JSON.
    #[arg(long)]
    pub public_output: PathBuf,
    /// Destination for the private unbound identity JSON.
    #[arg(long)]
    pub private_output: PathBuf,
    /// Overwrite existing output files.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub force: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationDkgRosterArgs {
    /// Public member enrollment JSON. Repeat once for every member.
    #[arg(long = "member", required = true)]
    pub members: Vec<PathBuf>,
    /// BLS signature threshold.
    #[arg(long)]
    pub threshold: u16,
    /// Matching payment observations required; defaults to the BFT consensus threshold.
    #[arg(long)]
    pub payment_observation_threshold: Option<u16>,
    /// Currency unit for the first DKG keyset.
    #[arg(long, default_value = "sat")]
    pub unit: String,
    /// Comma-separated denominations for the first DKG keyset.
    #[arg(long, value_delimiter = ',', default_value = "1,2,4,8,16,32,64,128")]
    pub amounts: Vec<u64>,
    /// Input fee in parts per thousand for the first DKG keyset.
    #[arg(long, default_value_t = 0)]
    pub input_fee_ppk: u64,
    /// Optional final expiry timestamp for the first DKG keyset.
    #[arg(long)]
    pub final_expiry: Option<u64>,
    /// Destination for the shared setup-only public federation config.
    #[arg(long)]
    pub public_output: PathBuf,
    /// Destination for the shared `[federation.dkg]` TOML snippet.
    #[arg(long)]
    pub ceremony_config_output: PathBuf,
    /// Overwrite existing output files.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub force: bool,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationDkgBindArgs {
    /// Shared setup-only public federation config JSON.
    #[arg(long)]
    pub public_config: PathBuf,
    /// Private unbound identity JSON generated by `dkg-member`.
    #[arg(long)]
    pub private_identity: PathBuf,
    /// Destination for the roster-bound private setup config.
    #[arg(long)]
    pub output: PathBuf,
    /// Overwrite an existing output file.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub force: bool,
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

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Args)]
pub struct FederationQuarantinePaymentObservationArgs {
    /// Shared public federation config JSON path.
    #[arg(long)]
    pub public_config: Option<PathBuf>,
    /// Private member config JSON path used to sign operation submit requests.
    #[arg(long)]
    pub private_config: Option<PathBuf>,
    /// Payment-observation context to quarantine.
    #[arg(long, value_enum)]
    pub context: FederationPaymentObservationContextArg,
    /// Quote ID whose payment observations should be quarantined.
    #[arg(long)]
    pub quote: String,
    /// Machine-readable reason for the quarantine.
    #[arg(long, value_enum, default_value_t = FederationPaymentObservationQuarantineReasonArg::ConflictingEvidence)]
    pub reason: FederationPaymentObservationQuarantineReasonArg,
    /// Accepted payment-observation operation ID that motivated the quarantine. Repeatable.
    #[arg(long = "evidence-operation-id")]
    pub evidence_operation_ids: Vec<String>,
    /// Member ID to submit to. Repeat to submit to multiple members. Defaults to every member.
    #[arg(long = "member")]
    pub members: Vec<u16>,
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

#[cfg(feature = "federation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FederationPaymentObservationQuarantineReasonArg {
    /// Matching observers submitted different payment evidence.
    ConflictingEvidence,
    /// Multiple terminal states were observed for the same quote.
    CompetingTerminalStates,
    /// Manual operator halt while evidence is inspected.
    OperatorHalt,
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
    /// Generate signed setup-route probe request fixtures for DKG route testing.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub dkg_setup_route_probes: bool,
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
    /// Submit a conflicting mint payment observation and quarantine it through private REST.
    QuarantineMintConflict,
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
    /// Fail if queried members with accepted operations lack quorum-backed catch-up certificates.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_catch_up_certificate_quorum: bool,
    /// Fail if queried members report or reveal equal-height digest mismatches.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_digest_alerts: bool,
    /// Fail if queried members report stale, conflicting, or quarantined payment observations.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_payment_observation_alerts: bool,
    /// Fail if queried members report invalid-operation alert buckets.
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub require_no_invalid_operation_alerts: bool,
}

#[cfg(all(test, feature = "federation"))]
mod tests {
    use super::*;

    #[test]
    fn parses_production_dkg_setup_commands() {
        let member = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-member",
            "--member-id",
            "2",
            "--public-mint-url",
            "https://mint2.example.com",
            "--federation-api-url",
            "https://fed2.example.com",
            "--public-output",
            "member-2-public.json",
            "--private-output",
            "member-2-private.json",
        ])
        .expect("dkg-member command parses");
        let Some(Commands::Federation(member)) = member.command else {
            panic!("expected federation command");
        };
        assert!(matches!(
            member.command,
            FederationCommand::DkgMember(FederationDkgMemberArgs { member_id: 2, .. })
        ));

        let roster = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-roster",
            "--member",
            "member-1-public.json",
            "--member",
            "member-2-public.json",
            "--threshold",
            "2",
            "--public-output",
            "dkg-setup-public.json",
            "--ceremony-config-output",
            "dkg-ceremony.toml",
        ])
        .expect("dkg-roster command parses");
        let Some(Commands::Federation(roster)) = roster.command else {
            panic!("expected federation command");
        };
        let FederationCommand::DkgRoster(roster) = roster.command else {
            panic!("expected dkg-roster command");
        };
        assert_eq!(roster.members.len(), 2);
        assert_eq!(roster.threshold, 2);

        assert!(CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-roster",
            "--member",
            "member-1-public.json",
            "--threshold",
            "1",
            "--payment-trust-model",
            "shared-backend-read-only",
            "--public-output",
            "dkg-setup-public.json",
            "--ceremony-config-output",
            "dkg-ceremony.toml",
        ])
        .is_err());

        let bind = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "dkg-bind",
            "--public-config",
            "dkg-setup-public.json",
            "--private-identity",
            "member-2-private.json",
            "--output",
            "member-2-bound.json",
        ])
        .expect("dkg-bind command parses");
        let Some(Commands::Federation(bind)) = bind.command else {
            panic!("expected federation command");
        };
        assert!(matches!(bind.command, FederationCommand::DkgBind(_)));
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
        assert_eq!(
            status.final_config_dir,
            Some(PathBuf::from("member-1/finalized"))
        );
        assert!(status.force);
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
        assert!(!trusted_dealer.dkg_setup_route_probes);
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
    fn parses_federation_quarantine_payment_observation_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "quarantine-payment-observation",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--context",
            "mint-quote",
            "--quote",
            "quote-1",
            "--reason",
            "competing-terminal-states",
            "--evidence-operation-id",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "--member",
            "2",
        ])
        .expect("quarantine command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::QuarantinePaymentObservation(quarantine) = federation.command else {
            panic!("expected federation quarantine command");
        };
        assert_eq!(
            quarantine.public_config,
            Some(PathBuf::from("federation-public.json"))
        );
        assert_eq!(
            quarantine.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(
            quarantine.context,
            FederationPaymentObservationContextArg::MintQuote
        );
        assert_eq!(quarantine.quote, "quote-1");
        assert_eq!(
            quarantine.reason,
            FederationPaymentObservationQuarantineReasonArg::CompetingTerminalStates
        );
        assert_eq!(
            quarantine.evidence_operation_ids,
            vec!["0000000000000000000000000000000000000000000000000000000000000001".to_string()]
        );
        assert_eq!(quarantine.members, vec![2]);
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
    fn parses_federation_quarantine_smoke_operations_subcommand() {
        let args = CLIArgs::try_parse_from([
            "cdk-mintd",
            "federation",
            "smoke-operations",
            "--public-config",
            "federation-public.json",
            "--private-config",
            "member-1/federation-private.json",
            "--scenario",
            "quarantine-mint-conflict",
        ])
        .expect("quarantine smoke operations command parses");

        let Some(Commands::Federation(federation)) = args.command else {
            panic!("expected federation command");
        };
        let FederationCommand::SmokeOperations(smoke) = federation.command else {
            panic!("expected federation smoke-operations command");
        };
        assert_eq!(smoke.public_config, PathBuf::from("federation-public.json"));
        assert_eq!(
            smoke.private_config,
            Some(PathBuf::from("member-1/federation-private.json"))
        );
        assert_eq!(
            smoke.scenario,
            FederationSmokeScenario::QuarantineMintConflict
        );
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
            "--require-catch-up-certificate-quorum",
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
        assert!(status.require_catch_up_certificate_quorum);
        assert!(status.require_no_digest_alerts);
        assert!(status.require_no_payment_observation_alerts);
        assert!(status.require_no_invalid_operation_alerts);
    }
}
