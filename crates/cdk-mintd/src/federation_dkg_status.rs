use std::collections::BTreeSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgFinalizedMemberConfig,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgSetupMethod, FederationDkgTranscriptSignatureRequest, MemberConfigPrivate,
    MemberId,
};

use crate::cli::{CLIArgs, FederationDkgStatusArgs};

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
    if args.from_database {
        let database_records = read_database_records(cli_args).await?;
        merge_dkg_records(
            &mut keyset_shares,
            &mut transcript_signatures,
            database_records.clone(),
        );
        let setup_summary = DkgSetupRecordSummary::from_records(&database_records);
        let report = dkg_status_report_with_setup_records(
            &public_config,
            &private_config,
            &keyset_shares,
            &transcript_signatures,
            &setup_summary,
        );
        print!("{}", report.text);

        if args.require_ready && !report.ready {
            bail!("federation DKG finalization is not ready");
        }

        return Ok(());
    }

    let report = dkg_status_report(
        &public_config,
        &private_config,
        &keyset_shares,
        &transcript_signatures,
    );
    print!("{}", report.text);

    if args.require_ready && !report.ready {
        bail!("federation DKG finalization is not ready");
    }

    Ok(())
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
    commitments: Vec<FederationDkgParticipantCommitment>,
    reveals: Vec<FederationDkgParticipantCommitmentReveal>,
    secret_share_contributions: Vec<FederationDkgSecretShareRequest>,
    keyset_shares: Vec<FederationDkgMemberKeysetShare>,
    transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
}

fn merge_dkg_records(
    keyset_shares: &mut Vec<FederationDkgMemberKeysetShare>,
    transcript_signatures: &mut Vec<FederationDkgTranscriptSignatureRequest>,
    records: DkgCompletionRecords,
) {
    extend_unique(keyset_shares, records.keyset_shares);
    extend_unique(transcript_signatures, records.transcript_signatures);
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
    commitments: usize,
    reveals: usize,
    secret_share_contributions: usize,
    commitment_members: BTreeSet<MemberId>,
    reveal_members: BTreeSet<MemberId>,
    secret_share_senders: BTreeSet<MemberId>,
    secret_share_receivers: BTreeSet<MemberId>,
}

impl DkgSetupRecordSummary {
    fn from_records(records: &DkgCompletionRecords) -> Self {
        Self {
            commitments: records.commitments.len(),
            reveals: records.reveals.len(),
            secret_share_contributions: records.secret_share_contributions.len(),
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

fn dkg_status_report(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
) -> DkgStatusReport {
    dkg_status_report_with_setup_records(
        setup_public_config,
        local_private_config,
        keyset_shares,
        transcript_signatures,
        &DkgSetupRecordSummary::default(),
    )
}

fn dkg_status_report_with_setup_records(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    keyset_shares: &[FederationDkgMemberKeysetShare],
    transcript_signatures: &[FederationDkgTranscriptSignatureRequest],
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
        "setup_transcript={} setup_keysets={} keyset_share_records={} transcript_signature_records={}\n",
        setup_transcript,
        setup_public_config.keysets.len(),
        keyset_shares.len(),
        transcript_signatures.len()
    ));
    text.push_str(&format!(
        "keyset_share_ids={} transcript_hashes={} transcript_signers={}\n",
        display_string_set(&keyset_ids),
        display_string_set(&transcript_hashes),
        display_member_set(&transcript_signers)
    ));
    if setup_records != &DkgSetupRecordSummary::default() {
        text.push_str(&format!(
            "setup_records commitments={} reveals={} secret_share_contributions={} commitment_members={} reveal_members={} secret_share_senders={} secret_share_receivers={}\n",
            setup_records.commitments,
            setup_records.reveals,
            setup_records.secret_share_contributions,
            display_member_set(&setup_records.commitment_members),
            display_member_set(&setup_records.reveal_members),
            display_member_set(&setup_records.secret_share_senders),
            display_member_set(&setup_records.secret_share_receivers)
        ));
    }

    if !setup_public_config.keysets.is_empty()
        && !local_private_config.keyset_secret_shares.is_empty()
    {
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

#[cfg(test)]
mod tests;
