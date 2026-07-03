use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use cdk_common::federation::{FederationConfig, MemberConfigPrivate};
use cdk_common::nuts::nut02::KeySetVersion;

use crate::cli::{CLIArgs, FederationInspectArgs};
use crate::config;

pub(crate) fn run_inspect_command(cli_args: &CLIArgs, args: &FederationInspectArgs) -> Result<()> {
    let (public_config_path, private_config_path) = inspect_config_paths(cli_args, args)?;
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

fn inspect_config_paths(
    cli_args: &CLIArgs,
    args: &FederationInspectArgs,
) -> Result<(PathBuf, Option<PathBuf>)> {
    if let Some(public_config) = &args.public_config {
        return Ok((public_config.clone(), args.private_config.clone()));
    }

    let config_path = cli_args.config.as_deref().ok_or_else(|| {
        anyhow!(
            "federation inspect needs --public-config or a mintd --config with [federation] paths"
        )
    })?;
    let settings = config::Settings::try_new(Some(config_path))
        .with_context(|| format!("failed to load mint config {}", config_path.display()))?;
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
                "- valid=true member={} keysets={} total_amounts={}\n",
                private_config.member_id,
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

fn keyset_version_label(version: KeySetVersion) -> &'static str {
    match version {
        KeySetVersion::Version00 => "v1",
        KeySetVersion::Version01 => "v2",
        KeySetVersion::Version02 => "v3",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerConfigBundle,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::nut01::BlsSecretKey;
    use cdk_common::{Amount, CurrencyUnit, SecretKey};

    use super::*;

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("fed url"),
            identity_secret(member_id),
        )
    }

    fn inspect_fixture() -> TrustedDealerConfigBundle {
        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("params"),
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                7,
                Some(123_456),
                BTreeMap::from([
                    (
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                        ],
                    ),
                    (
                        Amount::from(2),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            BlsSecretKey::from_reduced_bytes(&[5; 32]),
                        ],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer bundle")
    }

    #[test]
    fn inspect_report_summarizes_public_config() {
        let bundle = inspect_fixture();
        let report = inspect_report(&bundle.public_config, None).expect("inspect report");
        let keyset = &bundle.public_config.keysets[0];

        assert!(report.contains(&format!(
            "Federation {}",
            bundle.public_config.federation_id
        )));
        assert!(report.contains("members=3 signature_threshold=2 consensus_threshold=3"));
        assert!(report.contains("member 1 public_mint_url=https://mint1.example.com"));
        assert!(report.contains("federation_api_url=https://fed1.example.com"));
        assert!(report.contains(&format!("keyset {}", keyset.id)));
        assert!(report.contains("unit=sat active=true version=v3"));
        assert!(report.contains("input_fee_ppk=7 final_expiry=123456 amounts=[1,2]"));
        assert!(report.contains("Local member config\n- not provided"));
    }

    #[test]
    fn inspect_report_validates_private_config_without_leaking_fields() {
        let bundle = inspect_fixture();
        let private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config");
        let report =
            inspect_report(&bundle.public_config, Some(private_config)).expect("inspect report");

        assert!(report.contains("Local member config\n- valid=true member=1"));
        assert!(report.contains("keysets=1 total_amounts=2"));
        assert!(!report.contains("identity_secret_key"));
        assert!(!report.contains("keyset_secret_shares"));
        assert!(!report.contains("BlsSecretKeyShare"));
    }

    #[test]
    fn inspect_report_rejects_mismatched_private_config() {
        let bundle = inspect_fixture();
        let mut private_config = bundle
            .private_configs
            .get(&member_id(1))
            .expect("private config")
            .clone();
        private_config.member_id = member_id(42);

        assert!(inspect_report(&bundle.public_config, Some(&private_config)).is_err());
    }
}
