#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::collections::BTreeMap;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::path::{Path, PathBuf};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::str::FromStr;

#[cfg(all(feature = "federation", feature = "fakewallet"))]
use anyhow::{anyhow, Context};
#[cfg(feature = "federation")]
use anyhow::{bail, Result};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use bip39::Mnemonic;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::federation::{
    generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::mint_url::MintUrl;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::nuts::nut01::BlsSecretKey;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::{Amount, CurrencyUnit, SecretKey};

#[cfg(feature = "federation")]
use crate::cli::FederationTrustedDealerArgs;

#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PUBLIC_CONFIG_FILE: &str = "federation-public.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PRIVATE_CONFIG_FILE: &str = "federation-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_FILE: &str = "config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerArtifacts {
    pub(crate) public_config_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerMemberArtifacts {
    pub(crate) config_path: PathBuf,
    pub(crate) private_config_path: PathBuf,
    pub(crate) work_dir: PathBuf,
}

#[cfg(feature = "federation")]
pub(crate) fn run_trusted_dealer_command(args: &FederationTrustedDealerArgs) -> Result<()> {
    #[cfg(not(feature = "fakewallet"))]
    {
        let _ = args;
        bail!("the federation trusted-dealer generator requires the fakewallet feature");
    }

    #[cfg(feature = "fakewallet")]
    {
        let artifacts = generate_trusted_dealer_artifacts(args)?;
        println!(
            "Wrote federation public config: {}",
            artifacts.public_config_path.display()
        );
        for (member_id, member) in artifacts.member_artifacts {
            println!(
                "Wrote member {} config: {}",
                member_id,
                member.config_path.display()
            );
            println!(
                "Run member {} with work dir: {}",
                member_id,
                member.work_dir.display()
            );
        }

        Ok(())
    }
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
pub(crate) fn generate_trusted_dealer_artifacts(
    args: &FederationTrustedDealerArgs,
) -> Result<TrustedDealerArtifacts> {
    validate_trusted_dealer_args(args)?;

    let output_dir = create_output_dir(&args.output_dir)?;
    write_generated_gitignore(&output_dir, args.force)?;

    let threshold_params = ThresholdParams::with_bft_consensus(args.members, args.threshold)?;
    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation unit {}", args.unit))?;
    let members = trusted_dealer_members(args.members, &args.host, args.port_start)?;
    let keyset_spec = trusted_dealer_keyset_spec(
        unit.clone(),
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
    )?;

    let bundle = generate_trusted_dealer_configs(threshold_params, members, vec![keyset_spec])
        .context("failed to generate trusted-dealer federation configs")?;
    validate_bundle(&bundle)?;

    let public_config_path = output_dir.join(PUBLIC_CONFIG_FILE);
    write_json_file(&public_config_path, &bundle.public_config, args.force)?;

    let mut member_artifacts = BTreeMap::new();
    for (member_id, private_config) in &bundle.private_configs {
        let member = bundle
            .public_config
            .member(*member_id)
            .ok_or_else(|| anyhow!("generated bundle is missing member {}", member_id))?;
        let member_dir = output_dir.join(format!("member-{}", member_id.get()));
        std::fs::create_dir_all(&member_dir)
            .with_context(|| format!("failed to create {}", member_dir.display()))?;
        let private_config_path = member_dir.join(PRIVATE_CONFIG_FILE);
        write_json_file(&private_config_path, private_config, args.force)?;

        let work_dir = member_dir.join("work");
        let config_path = member_dir.join(MEMBER_CONFIG_FILE);
        let member_url = member.public_mint_url.to_string();
        let config_toml = member_config_toml(
            member_id,
            &args.host,
            &member_url,
            &member_url,
            args.port_start
                .checked_add(member_id.get() - 1)
                .ok_or_else(|| anyhow!("member ports overflow u16"))?,
            &public_config_path,
            &private_config_path,
            &config_path,
            &work_dir,
            &unit,
        )?;
        write_text_file(&config_path, &config_toml, args.force)?;

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                private_config_path,
                work_dir,
            },
        );
    }

    Ok(TrustedDealerArtifacts {
        public_config_path,
        member_artifacts,
    })
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn validate_trusted_dealer_args(args: &FederationTrustedDealerArgs) -> Result<()> {
    if args.members == 0 {
        bail!("federation trusted-dealer generation needs at least one member");
    }
    if args.threshold == 0 || args.threshold > args.members {
        bail!(
            "invalid federation threshold {} for {} members",
            args.threshold,
            args.members
        );
    }
    if args.amounts.is_empty() {
        bail!("federation trusted-dealer generation needs at least one amount");
    }
    if args.amounts.iter().any(|amount| *amount == 0) {
        bail!("federation trusted-dealer amounts must be non-zero");
    }
    let last_port_offset = args.members.saturating_sub(1);
    args.port_start
        .checked_add(last_port_offset)
        .ok_or_else(|| anyhow!("member ports overflow u16"))?;

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn create_output_dir(output_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    output_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", output_dir.display()))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_generated_gitignore(output_dir: &Path, force: bool) -> Result<()> {
    write_text_file(
        &output_dir.join(GENERATED_GITIGNORE_FILE),
        "# Generated local federation material may contain secrets.\n*\n",
        force,
    )
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn trusted_dealer_members(
    member_count: u16,
    host: &str,
    port_start: u16,
) -> Result<Vec<TrustedDealerMember>> {
    (1..=member_count)
        .map(|id| {
            let member_id = MemberId::new(id)?;
            let port = port_start
                .checked_add(id - 1)
                .ok_or_else(|| anyhow!("member ports overflow u16"))?;
            let url = MintUrl::from_str(&format!("http://{host}:{port}"))
                .with_context(|| format!("invalid generated URL for member {member_id}"))?;
            Ok(TrustedDealerMember::new(
                member_id,
                url.clone(),
                url,
                SecretKey::generate(),
            ))
        })
        .collect()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn trusted_dealer_keyset_spec(
    unit: CurrencyUnit,
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    threshold: u16,
    amounts: &[u64],
) -> Result<TrustedDealerKeysetSpec> {
    let mut amount_coefficients = BTreeMap::new();
    for amount in amounts {
        let coefficients = random_bls_coefficients(threshold)?;
        if amount_coefficients
            .insert(Amount::from(*amount), coefficients)
            .is_some()
        {
            bail!("duplicate federation amount {amount}");
        }
    }

    Ok(TrustedDealerKeysetSpec::new(
        unit,
        true,
        input_fee_ppk,
        final_expiry,
        amount_coefficients,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn random_bls_coefficients(threshold: u16) -> Result<Vec<BlsSecretKey>> {
    (0..threshold)
        .map(|_| {
            let secret_key = SecretKey::generate_bls();
            Ok(secret_key.as_bls()?.clone())
        })
        .collect()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn validate_bundle(bundle: &TrustedDealerConfigBundle) -> Result<()> {
    bundle
        .public_config
        .validate()
        .context("generated public federation config is invalid")?;
    for private_config in bundle.private_configs.values() {
        private_config
            .validate_against_public(&bundle.public_config)
            .context("generated private federation config is invalid")?;
    }

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_config_toml(
    member_id: &MemberId,
    listen_host: &str,
    mint_url: &str,
    federation_url: &str,
    listen_port: u16,
    public_config_path: &Path,
    private_config_path: &Path,
    config_path: &Path,
    work_dir: &Path,
    unit: &CurrencyUnit,
) -> Result<String> {
    let mnemonic = Mnemonic::generate(12)
        .map_err(|err| anyhow!("failed to generate member mnemonic: {err}"))?
        .to_string();
    let public_config_path = path_to_string(public_config_path);
    let private_config_path = path_to_string(private_config_path);
    let config_path = path_to_string(config_path);
    let work_dir = path_to_string(work_dir);
    let unit = unit.to_string().to_lowercase();

    Ok(format!(
        r#"# Generated by cdk-mintd federation trusted-dealer.
# Contains local test secrets. Do not commit this file.
# Start this member with:
# cdk-mintd --work-dir {work_dir} --config {config_path}

[info]
url = {mint_url}
listen_host = {listen_host}
listen_port = {listen_port}
mnemonic = {mnemonic}
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = {name}
description = {description}

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
unit = {unit}
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000

[fake_wallet]
supported_units = [{unit}]
fee_percent = 0.0
reserve_fee_min = 1
min_delay_time = 0
max_delay_time = 0

[federation]
enabled = true
public_config_path = {public_config_path}
private_config_path = {private_config_path}
"#,
        work_dir = toml_string(&work_dir)?,
        config_path = toml_string(&config_path)?,
        mint_url = toml_string(mint_url)?,
        listen_host = toml_string(listen_host)?,
        mnemonic = toml_string(&mnemonic)?,
        name = toml_string(&format!("CDK federation member {}", member_id.get()))?,
        description = toml_string(&format!(
            "Local fake-wallet federation member {} at {}",
            member_id.get(),
            federation_url
        ))?,
        unit = toml_string(&unit)?,
        public_config_path = toml_string(&public_config_path)?,
        private_config_path = toml_string(&private_config_path)?,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn toml_string(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{json}\n"), force)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite generated federation material",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(all(test, feature = "federation", feature = "fakewallet"))]
mod tests {
    use super::*;
    use crate::config;
    use cdk_common::federation::{FederationConfig, MemberConfigPrivate};

    fn temp_output_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cdk_mintd_federation_{name}_{}",
            std::process::id()
        ))
    }

    fn generator_args(output_dir: PathBuf) -> FederationTrustedDealerArgs {
        FederationTrustedDealerArgs {
            output_dir,
            members: 3,
            threshold: 2,
            unit: "sat".to_string(),
            amounts: vec![1, 2, 4],
            input_fee_ppk: 0,
            final_expiry: None,
            host: "127.0.0.1".to_string(),
            port_start: 8085,
            force: false,
        }
    }

    #[test]
    fn trusted_dealer_artifacts_are_written_and_validated() {
        let output_dir = temp_output_dir("artifacts");
        let _ = std::fs::remove_dir_all(&output_dir);
        let artifacts = generate_trusted_dealer_artifacts(&generator_args(output_dir.clone()))
            .expect("trusted dealer artifacts");

        let public_json =
            std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
        let public_config: FederationConfig =
            serde_json::from_str(&public_json).expect("public config json");
        public_config.validate().expect("public config validates");
        assert_eq!(public_config.threshold_params.member_count, 3);
        assert_eq!(public_config.threshold_params.signature_threshold, 2);
        assert_eq!(public_config.keysets.len(), 1);

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            let private_json = std::fs::read_to_string(&member_artifacts.private_config_path)
                .expect("private config file");
            let private_config: MemberConfigPrivate =
                serde_json::from_str(&private_json).expect("private config json");
            assert_eq!(private_config.member_id, *member_id);
            private_config
                .validate_against_public(&public_config)
                .expect("private config validates");

            let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
                .expect("member config toml");
            assert!(
                settings
                    .federation
                    .as_ref()
                    .expect("federation settings")
                    .enabled
            );
            assert_eq!(settings.info.listen_port, 8084 + member_id.get());
        }

        let _ = std::fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn trusted_dealer_artifacts_refuse_to_overwrite_without_force() {
        let output_dir = temp_output_dir("overwrite");
        let _ = std::fs::remove_dir_all(&output_dir);
        let args = generator_args(output_dir.clone());
        generate_trusted_dealer_artifacts(&args).expect("first trusted dealer artifacts");

        let err = generate_trusted_dealer_artifacts(&args)
            .expect_err("second generation should refuse overwrite");
        assert!(err.to_string().contains("already exists"));

        let mut forced_args = args;
        forced_args.force = true;
        generate_trusted_dealer_artifacts(&forced_args).expect("forced generation");

        let _ = std::fs::remove_dir_all(&output_dir);
    }
}
