use std::collections::BTreeSet;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgActivationRequest,
    FederationDkgCeremonyRecord, FederationDkgCeremonyState, FederationDkgDriverPhase,
    FederationDkgDriverRecords, FederationDkgFinalizedMemberConfig, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest, FederationDkgSetupMethod,
    FederationDkgTranscriptSignatureRequest, FederationFrostApplicationConfig,
    FederationFrostRootPublicConfig, FederationFrostWalletConfig,
    FederationOnchainConfirmationPolicy, MemberConfigPrivate, MemberId,
};

use crate::cli::{CLIArgs, FederationDkgStatusArgs};

const FINAL_PUBLIC_CONFIG_FILE: &str = "federation-public.json";
const FINAL_PRIVATE_CONFIG_FILE: &str = "federation-private.json";

pub(crate) async fn run_dkg_status_command(
    cli_args: &CLIArgs,
    args: &FederationDkgStatusArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation dkg-status",
    )?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;
    public_config.validate()?;
    private_config.validate_against_public(&public_config)?;

    let mut keyset_shares = read_record_files(&args.keyset_shares, "DKG keyset-share record")?;
    let mut transcript_signatures = read_record_files(
        &args.transcript_signatures,
        "DKG transcript-signature record",
    )?;
    let mut activations = read_record_files(&args.activations, "DKG activation record")?;
    if args.from_database {
        let database_records = read_database_records(cli_args).await?;
        if args.progress_only {
            let setup_summary = DkgSetupRecordSummary::from_record_counts(&database_records);
            print!(
                "{}",
                dkg_progress_report(&public_config, &private_config, &setup_summary)
            );
            return Ok(());
        }
        let current_records = database_records.records_for_current_ceremony()?;
        let frost_roots = current_records.frost_roots.clone();
        merge_dkg_records(
            &mut keyset_shares,
            &mut transcript_signatures,
            &mut activations,
            current_records,
        );
        let setup_summary =
            DkgSetupRecordSummary::from_records(&database_records, &public_config, &private_config);
        let report = dkg_status_report_with_setup_records(
            &public_config,
            &private_config,
            &keyset_shares,
            &transcript_signatures,
            &activations,
            &setup_summary,
        );
        let written_configs = maybe_write_finalized_configs(
            args,
            &public_config,
            &private_config,
            &keyset_shares,
            &transcript_signatures,
            &activations,
            &frost_roots,
        )?;
        print!("{}", report.text);
        print_written_config_status(written_configs.as_ref());

        if args.require_ready && !report.ready {
            bail!("federation DKG finalization is not ready");
        }

        return Ok(());
    }

    let report = dkg_status_report_with_activations(
        &public_config,
        &private_config,
        &keyset_shares,
        &transcript_signatures,
        &activations,
    );
    let written_configs = maybe_write_finalized_configs(
        args,
        &public_config,
        &private_config,
        &keyset_shares,
        &transcript_signatures,
        &activations,
        &[],
    )?;
    print!("{}", report.text);
    print_written_config_status(written_configs.as_ref());

    if args.require_ready && !report.ready {
        bail!("federation DKG finalization is not ready");
    }

    Ok(())
}

fn maybe_write_finalized_configs(
    args: &FederationDkgStatusArgs,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
    activations: &[FederationDkgActivationRequest],
    frost_roots: &[FederationFrostRootPublicConfig],
) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(output_dir) = args.final_config_dir.as_deref() else {
        return Ok(None);
    };
    let mut finalized = finalized_member_config(
        public_config,
        private_config,
        keyset_shares,
        transcript_signatures,
        activations,
    )?;
    if let Some(bootstrap) = FrostCdkBootstrap::from_args(args)? {
        attach_initial_frost_cdk_wallet(&mut finalized, frost_roots, bootstrap)?;
    }
    write_finalized_member_config(output_dir, &finalized, args.force).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrostCdkBootstrap {
    network: bitcoin::Network,
    minimum_confirmations: u32,
    fee_policy_version: u16,
    application_key_epoch: u64,
}

impl FrostCdkBootstrap {
    fn from_args(args: &FederationDkgStatusArgs) -> Result<Option<Self>> {
        let values = (
            args.frost_cdk_network.as_deref(),
            args.frost_cdk_minimum_confirmations,
            args.frost_cdk_fee_policy_version,
            args.frost_cdk_application_key_epoch,
        );
        let (network, minimum_confirmations, fee_policy_version, application_key_epoch) =
            match values {
                (None, None, None, None) => return Ok(None),
                (Some(network), Some(confirmations), Some(fee_policy), Some(epoch)) => {
                    (network, confirmations, fee_policy, epoch)
                }
                _ => bail!(
                    "FROST CDK bootstrap requires --frost-cdk-network, \
                     --frost-cdk-minimum-confirmations, --frost-cdk-fee-policy-version, and \
                     --frost-cdk-application-key-epoch together"
                ),
            };
        let network = match network {
            "bitcoin" | "mainnet" => bitcoin::Network::Bitcoin,
            "testnet" => bitcoin::Network::Testnet,
            "signet" => bitcoin::Network::Signet,
            "regtest" => bitcoin::Network::Regtest,
            other => bail!("unknown FROST CDK Bitcoin network {other}"),
        };
        FederationOnchainConfirmationPolicy::new(minimum_confirmations)?;
        if fee_policy_version == 0 || application_key_epoch == 0 {
            bail!("FROST CDK fee-policy and application epochs must be non-zero");
        }
        Ok(Some(Self {
            network,
            minimum_confirmations,
            fee_policy_version,
            application_key_epoch,
        }))
    }
}

fn attach_initial_frost_cdk_wallet(
    finalized: &mut FederationDkgFinalizedMemberConfig,
    frost_roots: &[FederationFrostRootPublicConfig],
    bootstrap: FrostCdkBootstrap,
) -> Result<()> {
    if finalized.public_config.frost_wallet.is_some() {
        bail!("initial FROST CDK bootstrap requires a config without an existing wallet");
    }
    let [root] = frost_roots else {
        bail!("initial FROST CDK bootstrap requires exactly one root from the completed dual DKG");
    };
    root.validate(&finalized.public_config)?;
    let application = FederationFrostApplicationConfig::new_cdk_onchain(
        &finalized.public_config,
        root,
        bootstrap.application_key_epoch,
        bootstrap.network,
        FederationOnchainConfirmationPolicy::new(bootstrap.minimum_confirmations)?,
        bootstrap.fee_policy_version,
    )?;
    finalized.public_config.frost_wallet = Some(FederationFrostWalletConfig::new(
        &finalized.public_config,
        root.clone(),
        application,
    )?);
    finalized.public_config.validate()?;
    finalized
        .private_config
        .validate_against_public(&finalized.public_config)?;
    Ok(())
}

fn finalized_member_config(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
    activations: &[FederationDkgActivationRequest],
) -> Result<FederationDkgFinalizedMemberConfig> {
    let (setup_public_config, finalized) =
        if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
            public_config.validate()?;
            private_config.validate_against_public(public_config)?;
            let mut setup_public_config = public_config.clone();
            setup_public_config.keysets.clear();
            setup_public_config.setup_transcript = None;
            (
                setup_public_config,
                FederationDkgFinalizedMemberConfig {
                    public_config: public_config.clone(),
                    private_config: private_config.clone(),
                },
            )
        } else {
            (
                public_config.clone(),
                FederationDkgFinalizedMemberConfig::from_records(
                    public_config,
                    private_config,
                    keyset_shares,
                    transcript_signatures,
                )?,
            )
        };
    if finalized
        .public_config
        .setup_transcript
        .as_ref()
        .is_some_and(|transcript| {
            transcript.payload.setup_method == FederationDkgSetupMethod::PedersenDkg
        })
    {
        finalized.validate_activation_records(&setup_public_config, activations)?;
    }
    Ok(finalized)
}

fn write_finalized_member_config(
    output_dir: &Path,
    finalized: &FederationDkgFinalizedMemberConfig,
    force: bool,
) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let public_path = output_dir.join(FINAL_PUBLIC_CONFIG_FILE);
    let private_path = output_dir.join(FINAL_PRIVATE_CONFIG_FILE);
    if !force {
        for path in [&public_path, &private_path] {
            if path.exists() {
                bail!(
                    "{} already exists; pass --force to replace finalized DKG config material",
                    path.display()
                );
            }
        }
    }

    write_final_config_file(&private_path, &finalized.private_config, force, true)?;
    write_final_config_file(&public_path, &finalized.public_config, force, false)?;
    Ok((public_path, private_path))
}

fn write_final_config_file<T>(path: &Path, value: &T, force: bool, private: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(if private { 0o600 } else { 0o644 });
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    serde_json::to_writer_pretty(&mut file, value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", path.display()))?;
    }

    Ok(())
}

fn print_written_config_status(paths: Option<&(PathBuf, PathBuf)>) {
    if paths.is_some() {
        println!(
            "final_configs_written=true public_file={FINAL_PUBLIC_CONFIG_FILE} private_file={FINAL_PRIVATE_CONFIG_FILE}"
        );
    }
}

async fn read_database_records(cli_args: &CLIArgs) -> Result<DkgCompletionRecords> {
    let work_dir = crate::get_work_directory(cli_args).await?;
    let settings = load_database_settings(&work_dir, cli_args)?;
    #[cfg(feature = "sqlcipher")]
    let password = Some(cli_args.password.clone());
    #[cfg(not(feature = "sqlcipher"))]
    let password = None;
    let (db, _, _) = crate::setup_database(&settings, &work_dir, password)
        .await
        .context("failed to open mint database for federation DKG status")?;

    Ok(DkgCompletionRecords {
        participant_states: db
            .get_federation_dkg_participant_states()
            .await
            .context("failed to read persisted local DKG participant states")?,
        commitments: db
            .get_federation_dkg_commitments()
            .await
            .context("failed to read persisted DKG commitment records")?,
        reveals: db
            .get_federation_dkg_reveals()
            .await
            .context("failed to read persisted DKG reveal records")?,
        secret_share_contributions: db
            .get_federation_dkg_secret_share_contributions()
            .await
            .context("failed to read persisted DKG secret-share contribution records")?,
        keyset_shares: db
            .get_federation_dkg_member_keyset_shares()
            .await
            .context("failed to read persisted DKG keyset-share records")?,
        transcript_signatures: db
            .get_federation_dkg_transcript_signatures()
            .await
            .context("failed to read persisted DKG transcript-signature records")?,
        activations: db
            .get_federation_dkg_activations()
            .await
            .context("failed to read persisted DKG activation records")?,
        ceremonies: db
            .get_federation_dkg_ceremonies()
            .await
            .context("failed to read persisted DKG ceremony lifecycle records")?,
        frost_roots: db
            .get_federation_frost_root_public_configs()
            .await
            .context("failed to read persisted FROST root public configs")?,
    })
}

fn load_database_settings(work_dir: &Path, cli_args: &CLIArgs) -> Result<crate::config::Settings> {
    let config_path = cli_args
        .config
        .clone()
        .unwrap_or_else(|| work_dir.join("config.toml"));
    let mut settings = if config_path.exists() {
        crate::config::Settings::try_new(Some(config_path.clone()))
            .with_context(|| format!("failed to load mint config {}", config_path.display()))?
    } else {
        crate::config::Settings::default()
    };

    apply_database_env_overrides(&mut settings)?;
    Ok(settings)
}

fn apply_database_env_overrides(settings: &mut crate::config::Settings) -> Result<()> {
    if let Ok(database) = env::var(crate::env_vars::DATABASE_ENV_VAR) {
        settings.database.engine = database.parse().map_err(anyhow::Error::msg)?;
    }

    if settings.database.engine == crate::config::DatabaseEngine::Postgres {
        settings.database.postgres = Some(
            settings
                .database
                .postgres
                .clone()
                .unwrap_or_default()
                .from_env(),
        );
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DkgCompletionRecords {
    participant_states: Vec<FederationDkgParticipantState>,
    commitments: Vec<FederationDkgParticipantCommitment>,
    reveals: Vec<FederationDkgParticipantCommitmentReveal>,
    secret_share_contributions: Vec<FederationDkgSecretShareRequest>,
    keyset_shares: Vec<FederationDkgMemberKeysetShare>,
    transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    activations: Vec<FederationDkgActivationRequest>,
    ceremonies: Vec<FederationDkgCeremonyRecord>,
    frost_roots: Vec<FederationFrostRootPublicConfig>,
}

impl DkgCompletionRecords {
    fn records_for_current_ceremony(&self) -> Result<Self> {
        if self.ceremonies.is_empty() {
            return Ok(self.clone());
        }
        let current = self
            .ceremonies
            .iter()
            .filter(|record| record.replacement_ceremony_id.is_none())
            .filter(|record| record.state != FederationDkgCeremonyState::Aborted)
            .collect::<Vec<_>>();
        if current.len() > 1 {
            bail!("multiple unreplaced DKG ceremony lifecycle records are durable");
        }
        let mut selected = self.clone();
        let Some(ceremony_id) = current.first().map(|record| record.ceremony_id) else {
            selected.participant_states.clear();
            selected.commitments.clear();
            selected.reveals.clear();
            selected.secret_share_contributions.clear();
            selected.keyset_shares.clear();
            selected.transcript_signatures.clear();
            selected.activations.clear();
            return Ok(selected);
        };
        selected
            .participant_states
            .retain(|state| state.ceremony_id() == ceremony_id);
        selected
            .commitments
            .retain(|commitment| commitment.ceremony_id == ceremony_id);
        selected
            .reveals
            .retain(|reveal| reveal.ceremony_id == ceremony_id);
        selected
            .secret_share_contributions
            .retain(|contribution| contribution.ceremony_id == ceremony_id);
        selected
            .keyset_shares
            .retain(|share| share.public_ceremony.ceremony_id() == ceremony_id);
        selected
            .activations
            .retain(|activation| activation.ceremony_id == ceremony_id);
        selected
            .frost_roots
            .retain(|root| root.ceremony_id == ceremony_id);
        Ok(selected)
    }
}

fn merge_dkg_records(
    keyset_shares: &mut Vec<FederationDkgMemberKeysetShare>,
    transcript_signatures: &mut Vec<FederationDkgTranscriptSignatureRequest>,
    activations: &mut Vec<FederationDkgActivationRequest>,
    records: DkgCompletionRecords,
) {
    extend_unique(keyset_shares, records.keyset_shares);
    extend_unique(transcript_signatures, records.transcript_signatures);
    extend_unique(activations, records.activations);
}

fn extend_unique<T>(target: &mut Vec<T>, source: Vec<T>)
where
    T: PartialEq,
{
    for item in source {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DkgSetupRecordSummary {
    ceremony_lifecycle: Vec<String>,
    participant_states: usize,
    commitments: usize,
    reveals: usize,
    secret_share_contributions: usize,
    keyset_shares: usize,
    transcript_signatures: usize,
    activations: usize,
    frost_root_public_configs: usize,
    participant_state_members: BTreeSet<MemberId>,
    participant_state_ceremonies: BTreeSet<String>,
    commitment_members: BTreeSet<MemberId>,
    reveal_members: BTreeSet<MemberId>,
    secret_share_senders: BTreeSet<MemberId>,
    secret_share_receivers: BTreeSet<MemberId>,
    driver_phase: Option<FederationDkgDriverPhase>,
    driver_error: Option<String>,
}

impl DkgSetupRecordSummary {
    fn from_record_counts(records: &DkgCompletionRecords) -> Self {
        Self {
            ceremony_lifecycle: records
                .ceremonies
                .iter()
                .map(|record| {
                    let successor = record
                        .replacement_ceremony_id
                        .map(|ceremony_id| ceremony_id.to_string())
                        .unwrap_or_else(|| "none".to_string());
                    format!(
                        "{}:{}:replacement={successor}",
                        record.ceremony_id,
                        record.state.as_str()
                    )
                })
                .collect(),
            participant_states: records.participant_states.len(),
            commitments: records.commitments.len(),
            reveals: records.reveals.len(),
            secret_share_contributions: records.secret_share_contributions.len(),
            keyset_shares: records.keyset_shares.len(),
            transcript_signatures: records.transcript_signatures.len(),
            activations: records.activations.len(),
            frost_root_public_configs: records.frost_roots.len(),
            participant_state_members: records
                .participant_states
                .iter()
                .map(FederationDkgParticipantState::member_id)
                .collect(),
            participant_state_ceremonies: records
                .participant_states
                .iter()
                .map(|state| state.ceremony_id().to_string())
                .collect(),
            commitment_members: records
                .commitments
                .iter()
                .map(|commitment| commitment.member_id)
                .collect(),
            reveal_members: records
                .reveals
                .iter()
                .map(|reveal| reveal.member_id)
                .collect(),
            secret_share_senders: records
                .secret_share_contributions
                .iter()
                .map(|contribution| contribution.sender)
                .collect(),
            secret_share_receivers: records
                .secret_share_contributions
                .iter()
                .map(|contribution| contribution.receiver)
                .collect(),
            driver_phase: None,
            driver_error: None,
        }
    }

    fn from_records(
        records: &DkgCompletionRecords,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> Self {
        let selected = match records.records_for_current_ceremony() {
            Ok(selected) => selected,
            Err(err) => {
                return Self {
                    driver_error: Some(redact_federation_operator_message(&err.to_string())),
                    ..Self::from_record_counts(records)
                };
            }
        };
        let local_participants = selected
            .participant_states
            .iter()
            .filter(|state| state.member_id() == private_config.member_id)
            .collect::<Vec<_>>();
        let (driver_phase, driver_error) = match local_participants.as_slice() {
            [] => (None, None),
            [participant] => {
                let driver_records = FederationDkgDriverRecords {
                    commitments: selected.commitments.clone(),
                    reveals: selected.reveals.clone(),
                    secret_share_contributions: selected.secret_share_contributions.clone(),
                    keyset_shares: selected.keyset_shares.clone(),
                    transcript_signatures: selected.transcript_signatures.clone(),
                    activations: selected.activations.clone(),
                };
                match driver_records.plan(public_config, private_config, participant) {
                    Ok(plan) => (Some(plan.phase), None),
                    Err(err) => (
                        None,
                        Some(redact_federation_operator_message(&err.to_string())),
                    ),
                }
            }
            _ => (
                None,
                Some("multiple local participant states require ceremony selection".to_string()),
            ),
        };

        Self {
            driver_phase,
            driver_error,
            ..Self::from_record_counts(records)
        }
    }
}

fn read_record_files<T>(paths: &[PathBuf], label: &str) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    paths
        .iter()
        .map(|path| crate::read_federation_json(path, label))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DkgStatusReport {
    text: String,
    ready: bool,
}

#[cfg(test)]
fn dkg_status_report(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
) -> DkgStatusReport {
    dkg_status_report_with_activations(
        setup_public_config,
        local_private_config,
        keyset_shares,
        transcript_signatures,
        &[],
    )
}

fn dkg_status_report_with_activations(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
    activations: &[FederationDkgActivationRequest],
) -> DkgStatusReport {
    dkg_status_report_with_setup_records(
        setup_public_config,
        local_private_config,
        keyset_shares,
        transcript_signatures,
        activations,
        &DkgSetupRecordSummary::default(),
    )
}

fn dkg_progress_report(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    setup_records: &DkgSetupRecordSummary,
) -> String {
    let mut text = format!(
        "Federation {} DKG progress for member {}\n",
        setup_public_config.federation_id, local_private_config.member_id
    );
    append_setup_record_summary(&mut text, setup_records, "unchecked");
    text
}

fn dkg_status_report_with_setup_records(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
    activations: &[FederationDkgActivationRequest],
    setup_records: &DkgSetupRecordSummary,
) -> DkgStatusReport {
    let keyset_ids = keyset_shares
        .iter()
        .map(|share| share.keyset.id.to_string())
        .collect::<BTreeSet<_>>();
    let transcript_hashes = transcript_signatures
        .iter()
        .map(|signature| signature.transcript_hash.to_string())
        .collect::<BTreeSet<_>>();
    let transcript_signers = transcript_signatures
        .iter()
        .map(|signature| signature.signer)
        .collect::<BTreeSet<_>>();
    let activation_signers = activations
        .iter()
        .map(|activation| activation.signer)
        .collect::<BTreeSet<_>>();
    let setup_transcript = setup_public_config
        .setup_transcript
        .as_ref()
        .map(|transcript| setup_method_label(transcript.payload.setup_method))
        .unwrap_or("none");

    let mut text = String::new();
    text.push_str(&format!(
        "Federation {} DKG status for member {}\n",
        setup_public_config.federation_id, local_private_config.member_id
    ));
    text.push_str(&format!(
        "setup_transcript={} setup_keysets={} keyset_share_records={} transcript_signature_records={} activation_records={}\n",
        setup_transcript,
        setup_public_config.keysets.len(),
        keyset_shares.len(),
        transcript_signatures.len(),
        activations.len()
    ));
    text.push_str(&format!(
        "keyset_share_ids={} transcript_hashes={} transcript_signers={} activation_signers={}\n",
        display_string_set(&keyset_ids),
        display_string_set(&transcript_hashes),
        display_member_set(&transcript_signers),
        display_member_set(&activation_signers)
    ));
    if setup_records != &DkgSetupRecordSummary::default() {
        let phase = setup_records
            .driver_phase
            .map(dkg_driver_phase_label)
            .unwrap_or(if setup_records.driver_error.is_some() {
                "invalid"
            } else {
                "not_started"
            });
        append_setup_record_summary(&mut text, setup_records, phase);
        if let Some(reason) = setup_records.driver_error.as_deref() {
            text.push_str(&format!("driver_error={reason}\n"));
        }
    }

    if !setup_public_config.keysets.is_empty()
        && !local_private_config.keyset_secret_shares.is_empty()
    {
        let finalized = FederationDkgFinalizedMemberConfig {
            public_config: setup_public_config.clone(),
            private_config: local_private_config.clone(),
        };
        let mut stripped_setup = setup_public_config.clone();
        stripped_setup.keysets.clear();
        stripped_setup.setup_transcript = None;
        stripped_setup.frost_wallet = None;
        if setup_public_config
            .setup_transcript
            .as_ref()
            .is_some_and(|transcript| {
                transcript.payload.setup_method == FederationDkgSetupMethod::PedersenDkg
            })
        {
            if let Err(err) = finalized.validate_activation_records(&stripped_setup, activations) {
                text.push_str(&format!(
                    "finalized_ready=false reason={}\n",
                    redact_federation_operator_message(&err.to_string())
                ));
                return DkgStatusReport { text, ready: false };
            }
        }
        let transcript = setup_public_config.setup_transcript.as_ref();
        let transcript_hash = transcript
            .map(|transcript| transcript.transcript_hash.to_string())
            .unwrap_or_else(|| "none".to_string());
        let transcript_signers = transcript
            .map(|transcript| transcript.signatures.keys().copied().collect())
            .unwrap_or_default();
        let local_amounts = local_private_config
            .keyset_secret_shares
            .values()
            .map(|shares| shares.len())
            .sum::<usize>();
        text.push_str(&format!(
            "finalized_ready=true source=config final_keysets={} transcript_hash={} transcript_signers={} local_keysets={} local_amounts={}\n",
            setup_public_config.keysets.len(),
            transcript_hash,
            display_member_set(&transcript_signers),
            local_private_config.keyset_secret_shares.len(),
            local_amounts
        ));
        return DkgStatusReport { text, ready: true };
    }

    match FederationDkgFinalizedMemberConfig::from_records(
        setup_public_config,
        local_private_config,
        keyset_shares,
        transcript_signatures,
    ) {
        Ok(finalized) => {
            if let Err(err) =
                finalized.validate_activation_records(setup_public_config, activations)
            {
                text.push_str(&format!(
                    "finalized_ready=false reason={}\n",
                    redact_federation_operator_message(&err.to_string())
                ));
                return DkgStatusReport { text, ready: false };
            }
            let transcript = finalized.public_config.setup_transcript.as_ref();
            let transcript_hash = transcript
                .map(|transcript| transcript.transcript_hash.to_string())
                .unwrap_or_else(|| "none".to_string());
            let transcript_signers = transcript
                .map(|transcript| transcript.signatures.keys().copied().collect())
                .unwrap_or_default();
            let local_amounts = finalized
                .private_config
                .keyset_secret_shares
                .values()
                .map(|shares| shares.len())
                .sum::<usize>();
            text.push_str(&format!(
                "finalized_ready=true final_keysets={} transcript_hash={} transcript_signers={} local_keysets={} local_amounts={}\n",
                finalized.public_config.keysets.len(),
                transcript_hash,
                display_member_set(&transcript_signers),
                finalized.private_config.keyset_secret_shares.len(),
                local_amounts
            ));
            DkgStatusReport { text, ready: true }
        }
        Err(err) => {
            text.push_str(&format!(
                "finalized_ready=false reason={}\n",
                redact_federation_operator_message(&err.to_string())
            ));
            DkgStatusReport { text, ready: false }
        }
    }
}

fn append_setup_record_summary(
    text: &mut String,
    setup_records: &DkgSetupRecordSummary,
    phase: &str,
) {
    if !setup_records.ceremony_lifecycle.is_empty() {
        text.push_str(&format!(
            "ceremony_lifecycle={}\n",
            setup_records.ceremony_lifecycle.join(",")
        ));
    }
    text.push_str(&format!(
        "setup_records participant_states={} commitments={} reveals={} secret_share_contributions={} keyset_shares={} transcript_signatures={} activations={} frost_root_public_configs={} participant_state_members={} participant_state_ceremonies={} commitment_members={} reveal_members={} secret_share_senders={} secret_share_receivers={} driver_phase={phase}\n",
        setup_records.participant_states,
        setup_records.commitments,
        setup_records.reveals,
        setup_records.secret_share_contributions,
        setup_records.keyset_shares,
        setup_records.transcript_signatures,
        setup_records.activations,
        setup_records.frost_root_public_configs,
        display_member_set(&setup_records.participant_state_members),
        display_string_set(&setup_records.participant_state_ceremonies),
        display_member_set(&setup_records.commitment_members),
        display_member_set(&setup_records.reveal_members),
        display_member_set(&setup_records.secret_share_senders),
        display_member_set(&setup_records.secret_share_receivers),
    ));
}

fn display_string_set(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    values.iter().cloned().collect::<Vec<_>>().join(",")
}

fn display_member_set(values: &BTreeSet<MemberId>) -> String {
    if values.is_empty() {
        return "none".to_string();
    }

    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn setup_method_label(setup_method: FederationDkgSetupMethod) -> &'static str {
    match setup_method {
        FederationDkgSetupMethod::TrustedDealer => "trusted_dealer",
        FederationDkgSetupMethod::PedersenDkg => "pedersen_dkg",
    }
}

fn dkg_driver_phase_label(phase: FederationDkgDriverPhase) -> &'static str {
    match phase {
        FederationDkgDriverPhase::Commitment => "commitment",
        FederationDkgDriverPhase::Reveal => "reveal",
        FederationDkgDriverPhase::SecretShares => "share_delivery",
        FederationDkgDriverPhase::LocalKeysetShare => "local_keyset_share",
        FederationDkgDriverPhase::TranscriptSignature => "transcript_signature",
        FederationDkgDriverPhase::Activation => "activation",
        FederationDkgDriverPhase::FrostRound1 => "frost_round_1",
        FederationDkgDriverPhase::FrostRound2 => "frost_round_2",
        FederationDkgDriverPhase::FrostConfirmation => "frost_confirmation",
        FederationDkgDriverPhase::Complete => "complete",
    }
}

#[cfg(test)]
mod tests;
