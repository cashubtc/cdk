use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use cdk_common::federation::{FederationConfig, FederationDkgSetupMethod, MemberConfigPrivate};
use cdk_common::nuts::nut02::KeySetVersion;

use crate::cli::{CLIArgs, FederationInspectArgs};

pub(crate) async fn run_inspect_command(
    cli_args: &CLIArgs,
    args: &FederationInspectArgs,
) -> Result<()> {
    let (public_config_path, private_config_path) = inspect_config_paths(cli_args, args).await?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    public_config
        .validate()
        .context("invalid public federation config")?;

    let private_config = match private_config_path {
        Some(path) => {
            let private_config: MemberConfigPrivate =
                crate::read_federation_json(&path, "private federation config")?;
            private_config
                .validate_against_public(&public_config)
                .context("local member config does not match public federation config")?;
            Some(private_config)
        }
        None => None,
    };

    print!(
        "{}",
        inspect_report(&public_config, private_config.as_ref())?
    );
    Ok(())
}

async fn inspect_config_paths(
    cli_args: &CLIArgs,
    args: &FederationInspectArgs,
) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(public_config) = &args.public_config {
        return Ok((public_config.clone(), args.private_config.clone()));
    }

    let work_dir = crate::get_work_directory(cli_args).await?;
    let settings = crate::load_stored_settings(&work_dir, cli_args).await?;
    let federation = settings
        .federation
        .ok_or_else(|| anyhow!("mint config has no [federation] section"))?;
    let public_config = federation
        .public_config_path
        .ok_or_else(|| anyhow!("mint config [federation] has no public_config_path"))?;
    let private_config = args
        .private_config
        .clone()
        .or(federation.private_config_path);

    Ok((public_config, private_config))
}

fn inspect_report(
    public_config: &FederationConfig,
    private_config: Option<&MemberConfigPrivate>,
) -> Result<String> {
    public_config.validate()?;
    if let Some(private_config) = private_config {
        private_config.validate_against_public(public_config)?;
    }

    let mut report = String::new();
    report.push_str(&format!("Federation {}\n", public_config.federation_id));
    report.push_str(&format!(
        "members={} signature_threshold={} consensus_threshold={}\n",
        public_config.threshold_params.member_count,
        public_config.threshold_params.signature_threshold,
        public_config.threshold_params.consensus_threshold
    ));
    match &public_config.setup_transcript {
        Some(transcript) => {
            report.push_str(&format!(
                "setup_transcript={} hash={} signatures={}\n",
                setup_method_label(transcript.payload.setup_method),
                transcript.transcript_hash,
                transcript.signatures.len()
            ));
        }
        None => {
            report.push_str("setup_transcript=none\n");
        }
    }

    report.push_str("\nMembers\n");
    for member in public_config.members.values() {
        report.push_str(&format!(
            "- member {} public_mint_url={} federation_api_url={} identity_public_key={}\n",
            member.member_id,
            member.public_mint_url,
            member.federation_api_url,
            member.identity_public_key
        ));
    }

    report.push_str("\nAggregate keysets\n");
    for keyset in &public_config.keysets {
        let amounts = keyset
            .aggregate_keys
            .keys()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        report.push_str(&format!(
            "- keyset {} unit={} active={} version={} input_fee_ppk={} final_expiry={} amounts=[{}] aggregate_keys={} member_share_sets={}\n",
            keyset.id,
            keyset.unit,
            keyset.active,
            keyset_version_label(keyset.id.get_version()),
            keyset.input_fee_ppk,
            keyset
                .final_expiry
                .map(|expiry| expiry.to_string())
                .unwrap_or_else(|| "none".to_string()),
            amounts,
            keyset.aggregate_keys.len(),
            keyset.public_shares.len()
        ));
    }

    match private_config {
        Some(private_config) => {
            report.push_str("\nLocal member config\n");
            report.push_str(&format!(
                "- valid=true member={} federation_id={} setup_transcript_hash={} keysets={} total_amounts={}\n",
                private_config.member_id,
                private_config
                    .federation_id
                    .map(|federation_id| federation_id.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                private_config
                    .setup_transcript_hash
                    .map(|transcript_hash| transcript_hash.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                private_config.keyset_secret_shares.len(),
                private_config
                    .keyset_secret_shares
                    .values()
                    .map(|shares| shares.len())
                    .sum::<usize>()
            ));
        }
        None => {
            report.push_str("\nLocal member config\n- not provided\n");
        }
    }

    Ok(report)
}

fn setup_method_label(setup_method: FederationDkgSetupMethod) -> &'static str {
    match setup_method {
        FederationDkgSetupMethod::TrustedDealer => "trusted_dealer",
        FederationDkgSetupMethod::PedersenDkg => "pedersen_dkg",
    }
}

fn keyset_version_label(version: KeySetVersion) -> &'static str {
    match version {
        KeySetVersion::Version00 => "v1",
        KeySetVersion::Version01 => "v2",
        KeySetVersion::Version02 => "v3",
    }
}

#[cfg(test)]
mod tests;
