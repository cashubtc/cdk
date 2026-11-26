//! `cdk-mintd federation dkg-status`: argument parsing and the operator report
//! over DKG finalization in `cdk_federation::setup::finalize`.

use std::collections::BTreeSet;
use std::path::PathBuf;
#[cfg(feature = "federation-bark")]
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use cdk_common::federation::{
    redact_federation_operator_message, FederationConfig, FederationDkgActivationRequest,
    FederationDkgFinalizedMemberConfig, FederationDkgMemberKeysetShare, FederationDkgSetupMethod,
    FederationDkgTranscriptSignatureRequest, FederationFrostRootPublicConfig, MemberConfigPrivate,
    MemberId,
};
#[cfg(feature = "federation-bdk")]
use cdk_federation::setup::finalize::attach_initial_frost_cdk_wallet;
#[cfg(feature = "federation-bark")]
use cdk_federation::setup::finalize::{
    attach_initial_frost_bark_application, FederationFrostBarkBootstrap,
};
use cdk_federation::setup::finalize::{
    dkg_driver_phase_label, finalized_member_config, validate_frost_cdk_bootstrap,
    write_finalized_member_config, FederationDkgCompletionRecords, FederationDkgRecordSummary,
    FederationFrostCdkBootstrap, FINAL_PRIVATE_CONFIG_FILE, FINAL_PUBLIC_CONFIG_FILE,
};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::FederationBarkRecoverySetupConfig;

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
    )
    .await?;
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
            let setup_summary = FederationDkgRecordSummary::from_record_counts(&database_records);
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
        let setup_summary = FederationDkgRecordSummary::from_records(
            &database_records,
            &public_config,
            &private_config,
        );
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
    _frost_roots: &[FederationFrostRootPublicConfig],
) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(output_dir) = args.final_config_dir.as_deref() else {
        return Ok(None);
    };
    #[allow(unused_mut)]
    let mut finalized = finalized_member_config(
        public_config,
        private_config,
        keyset_shares,
        transcript_signatures,
        activations,
    )?;
    let cdk_bootstrap = frost_cdk_bootstrap_from_args(args)?;
    #[cfg(feature = "federation-bark")]
    let bark_bootstrap = frost_bark_bootstrap_from_args(args)?;
    #[cfg(not(feature = "federation-bark"))]
    let bark_bootstrap_requested = args.frost_bark_network.is_some()
        || args.frost_bark_ark_server_public_key.is_some()
        || args.frost_bark_application_key_epoch.is_some()
        || args.frost_bark_policy_config.is_some()
        || args.frost_bark_recovery_minimum_confirmations.is_some()
        || args.frost_bark_recovery_fee_policy_version.is_some();
    #[cfg(not(feature = "federation-bark"))]
    if bark_bootstrap_requested {
        bail!("initial Bark bootstrap requires the `federation-bark` feature");
    }
    #[cfg(feature = "federation-bdk")]
    if let Some(bootstrap) = cdk_bootstrap {
        attach_initial_frost_cdk_wallet(&mut finalized, _frost_roots, bootstrap)?;
    }
    #[cfg(not(feature = "federation-bdk"))]
    if cdk_bootstrap.is_some() {
        bail!("CDK on-chain bootstrap requires the `federation-bdk` feature");
    }
    #[cfg(feature = "federation-bark")]
    if let Some(bootstrap) = bark_bootstrap {
        let recovery = FederationBarkRecoverySetupConfig {
            application_key_epoch: bootstrap.application_key_epoch,
            network: bootstrap.network,
            minimum_confirmations: bootstrap.recovery_minimum_confirmations,
            fee_policy_version: bootstrap.recovery_fee_policy_version,
        };
        attach_initial_frost_bark_application(&mut finalized, _frost_roots, bootstrap, recovery)?;
    }
    write_finalized_member_config(output_dir, &finalized, args.force).map(Some)
}

#[cfg(feature = "federation-bark")]
fn frost_bark_bootstrap_from_args(
    args: &FederationDkgStatusArgs,
) -> Result<Option<FederationFrostBarkBootstrap>> {
    let values = (
        args.frost_bark_network.as_deref(),
        args.frost_bark_ark_server_public_key.as_deref(),
        args.frost_bark_application_key_epoch,
        args.frost_bark_policy_config.as_deref(),
        args.frost_bark_recovery_minimum_confirmations,
        args.frost_bark_recovery_fee_policy_version,
    );
    let (
        network,
        ark_server_public_key,
        application_key_epoch,
        policy_path,
        recovery_minimum_confirmations,
        recovery_fee_policy_version,
    ) = match values {
        (None, None, None, None, None, None) => return Ok(None),
        (
            Some(network),
            Some(server),
            Some(epoch),
            Some(policy),
            Some(confirmations),
            Some(fee_policy),
        ) => (network, server, epoch, policy, confirmations, fee_policy),
        _ => bail!(
            "FROST Bark bootstrap requires --frost-bark-network, \
             --frost-bark-ark-server-public-key, --frost-bark-application-key-epoch, and \
             --frost-bark-policy-config, --frost-bark-recovery-minimum-confirmations, and \
             --frost-bark-recovery-fee-policy-version together"
        ),
    };
    let network = parse_bitcoin_network(network, "FROST Bark")?;
    let ark_server_public_key = bitcoin::secp256k1::PublicKey::from_str(ark_server_public_key)
        .context("invalid compressed Ark server public key")?;
    if application_key_epoch == 0 {
        bail!("FROST Bark application epoch must be non-zero");
    }
    if recovery_minimum_confirmations == 0 || recovery_fee_policy_version == 0 {
        bail!("FROST Bark recovery confirmations and fee-policy version must be non-zero");
    }
    let policy = crate::read_federation_json(policy_path, "Bark public policy config")?;
    Ok(Some(FederationFrostBarkBootstrap {
        network,
        ark_server_public_key,
        application_key_epoch,
        policy,
        recovery_minimum_confirmations,
        recovery_fee_policy_version,
    }))
}

fn frost_cdk_bootstrap_from_args(
    args: &FederationDkgStatusArgs,
) -> Result<Option<FederationFrostCdkBootstrap>> {
    let values = (
        args.frost_cdk_network.as_deref(),
        args.frost_cdk_minimum_confirmations,
        args.frost_cdk_fee_policy_version,
        args.frost_cdk_application_key_epoch,
    );
    let (network, minimum_confirmations, fee_policy_version, application_key_epoch) = match values {
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
    let bootstrap = FederationFrostCdkBootstrap {
        network: parse_bitcoin_network(network, "FROST CDK")?,
        minimum_confirmations,
        fee_policy_version,
        application_key_epoch,
    };
    validate_frost_cdk_bootstrap(&bootstrap)?;
    Ok(Some(bootstrap))
}

fn parse_bitcoin_network(value: &str, application: &str) -> Result<bitcoin::Network> {
    match value {
        "bitcoin" | "mainnet" => Ok(bitcoin::Network::Bitcoin),
        "testnet" => Ok(bitcoin::Network::Testnet),
        "testnet4" => Ok(bitcoin::Network::Testnet4),
        "signet" => Ok(bitcoin::Network::Signet),
        "regtest" => Ok(bitcoin::Network::Regtest),
        other => bail!("unknown {application} Bitcoin network {other}"),
    }
}

fn print_written_config_status(paths: Option<&(PathBuf, PathBuf)>) {
    if paths.is_some() {
        println!(
            "final_configs_written=true public_file={FINAL_PUBLIC_CONFIG_FILE} private_file={FINAL_PRIVATE_CONFIG_FILE}"
        );
    }
}

async fn read_database_records(cli_args: &CLIArgs) -> Result<FederationDkgCompletionRecords> {
    let work_dir = crate::get_work_directory(cli_args).await?;
    let settings = crate::load_database_bootstrap_settings()?;
    #[cfg(feature = "sqlcipher")]
    let password = cli_args.password.clone();
    #[cfg(not(feature = "sqlcipher"))]
    let password = None;
    let crate::MintDatabases { localstore: db, .. } =
        crate::setup_database(&settings, &work_dir, password)
            .await
            .context("failed to open mint database for federation DKG status")?;

    FederationDkgCompletionRecords::read(&db).await
}

fn merge_dkg_records(
    keyset_shares: &mut Vec<FederationDkgMemberKeysetShare>,
    transcript_signatures: &mut Vec<FederationDkgTranscriptSignatureRequest>,
    activations: &mut Vec<FederationDkgActivationRequest>,
    records: FederationDkgCompletionRecords,
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
        &FederationDkgRecordSummary::default(),
    )
}

fn dkg_progress_report(
    setup_public_config: &FederationConfig,
    local_private_config: &MemberConfigPrivate,
    setup_records: &FederationDkgRecordSummary,
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
    setup_records: &FederationDkgRecordSummary,
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
    if setup_records != &FederationDkgRecordSummary::default() {
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
    setup_records: &FederationDkgRecordSummary,
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

#[cfg(test)]
mod tests;
