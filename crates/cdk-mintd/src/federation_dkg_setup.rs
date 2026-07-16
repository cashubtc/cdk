use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use bitcoin::hashes::{sha256, Hash};
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationConfig, FederationDkgCeremonyId, FederationDkgKeysetSpec,
    FederationPaymentObservationPolicy, MemberConfigPrivate, MemberConfigPublic, MemberId,
    ThresholdParams, FEDERATION_MAX_DKG_MEMBERS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::{Amount, CurrencyUnit, SecretKey};
use url::Url;

use crate::cli::{FederationDkgBindArgs, FederationDkgMemberArgs, FederationDkgRosterArgs};
use crate::federation_files::{write_json_file, write_private_json_file, write_text_file};
use crate::read_federation_json;

pub(crate) fn run_dkg_member_command(args: &FederationDkgMemberArgs) -> Result<()> {
    ensure_output_paths_available(&[&args.public_output, &args.private_output], args.force)?;
    let member_id = MemberId::new(args.member_id).context("invalid federation member ID")?;
    let public_mint_url =
        MintUrl::from_str(&args.public_mint_url).context("invalid public Cashu mint URL")?;
    let federation_api_url = MintUrl::from_str(&args.federation_api_url)
        .context("invalid private federation API URL")?;
    validate_enrollment_url(&public_mint_url, "public mint", false)?;
    validate_enrollment_url(&federation_api_url, "federation API", true)?;
    let identity_secret_key = SecretKey::generate();
    let public = MemberConfigPublic {
        member_id,
        public_mint_url,
        federation_api_url,
        identity_public_key: identity_secret_key.public_key(),
    };
    public
        .validate(member_id)
        .context("generated public member enrollment is invalid")?;
    let private = MemberConfigPrivate {
        federation_id: None,
        member_id,
        identity_secret_key,
        keyset_secret_shares: BTreeMap::new(),
        setup_transcript_hash: None,
    };

    write_json_file(&args.public_output, &public, args.force)?;
    write_private_json_file(&args.private_output, &private, args.force)?;
    println!("member_id={member_id}");
    println!("public_enrollment={}", args.public_output.display());
    println!("private_identity={}", args.private_output.display());
    Ok(())
}

pub(crate) fn run_dkg_roster_command(args: &FederationDkgRosterArgs) -> Result<()> {
    let mut members = BTreeMap::new();
    for path in &args.members {
        let member: MemberConfigPublic =
            read_federation_json(path, "public DKG member enrollment")?;
        member
            .validate(member.member_id)
            .with_context(|| format!("invalid public member enrollment {}", path.display()))?;
        validate_enrollment_url(&member.public_mint_url, "public mint", false)
            .with_context(|| format!("invalid public member enrollment {}", path.display()))?;
        validate_enrollment_url(&member.federation_api_url, "federation API", true)
            .with_context(|| format!("invalid public member enrollment {}", path.display()))?;
        let member_id = member.member_id;
        if members.insert(member_id, member).is_some() {
            bail!("duplicate public DKG enrollment for member {member_id}");
        }
    }
    let member_count =
        u16::try_from(members.len()).map_err(|_| anyhow!("federation member count exceeds u16"))?;
    if members.len() > FEDERATION_MAX_DKG_MEMBERS {
        bail!(
            "federation DKG roster has {} members; maximum is {FEDERATION_MAX_DKG_MEMBERS}",
            members.len()
        );
    }
    let threshold_params = ThresholdParams::with_bft_consensus(member_count, args.threshold)
        .context("invalid federation threshold parameters")?;
    let mut public_config = FederationConfig::new(threshold_params, members, Vec::new())
        .context("invalid setup-only federation roster")?;
    if let Some(payment_threshold) = args.payment_observation_threshold {
        public_config.payment_observation_policy =
            FederationPaymentObservationPolicy::member_quorum(payment_threshold);
    }
    public_config.checkpoint_policy = FederationCheckpointPolicy::new(
        args.checkpoint_interval_journal_operations,
        args.checkpoint_interval_finalized_items,
    )
    .context("invalid federation checkpoint policy")?;
    public_config
        .validate()
        .context("invalid federation roster policy")?;

    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation DKG unit {}", args.unit))?;
    let amounts = args
        .amounts
        .iter()
        .copied()
        .map(Amount::from)
        .collect::<BTreeSet<_>>();
    if amounts.len() != args.amounts.len() {
        bail!("federation DKG denominations contain duplicates");
    }
    let keyset_spec =
        FederationDkgKeysetSpec::new(unit, true, args.input_fee_ppk, args.final_expiry, amounts)
            .context("invalid federation DKG keyset policy")?;
    let ceremony_id = new_ceremony_id(&public_config, &keyset_spec)?;
    let ceremony_config = ceremony_config_toml(
        ceremony_id,
        &keyset_spec,
        federation_endpoints_are_loopback(&public_config),
    )?;

    ensure_output_paths_available(
        &[&args.public_output, &args.ceremony_config_output],
        args.force,
    )?;
    write_json_file(&args.public_output, &public_config, args.force)?;
    write_text_file(&args.ceremony_config_output, &ceremony_config, args.force)?;
    println!("federation_id={}", public_config.federation_id);
    println!("ceremony_id={ceremony_id}");
    println!("public_config={}", args.public_output.display());
    println!("ceremony_config={}", args.ceremony_config_output.display());
    Ok(())
}

pub(crate) fn run_dkg_bind_command(args: &FederationDkgBindArgs) -> Result<()> {
    if args.output == args.public_config {
        bail!("bound private output must not overwrite the public federation config");
    }
    ensure_output_paths_available(&[&args.output], args.force)?;
    let public_config: FederationConfig =
        read_federation_json(&args.public_config, "setup-only public federation config")?;
    public_config
        .validate()
        .context("invalid setup-only public federation config")?;
    if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
        bail!("dkg-bind requires a setup-only public config without keysets or transcript");
    }

    let mut private: MemberConfigPrivate =
        read_federation_json(&args.private_identity, "private DKG member identity")?;
    if !private.keyset_secret_shares.is_empty() || private.setup_transcript_hash.is_some() {
        bail!("private DKG identity must not contain BLS shares or a setup transcript hash");
    }
    match private.federation_id {
        Some(federation_id) if federation_id != public_config.federation_id => {
            bail!("private DKG identity is already bound to a different federation")
        }
        Some(_) | None => {}
    }
    private.federation_id = Some(public_config.federation_id);
    private
        .validate_against_public(&public_config)
        .context("private DKG identity does not match the public roster")?;

    write_private_json_file(&args.output, &private, args.force)?;
    println!("federation_id={}", public_config.federation_id);
    println!("member_id={}", private.member_id);
    println!("private_config={}", args.output.display());
    Ok(())
}

fn ensure_output_paths_available(paths: &[&std::path::PathBuf], force: bool) -> Result<()> {
    let unique = paths.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != paths.len() {
        bail!("federation setup output paths must be distinct");
    }
    if !force {
        for path in paths {
            if path.exists() {
                bail!(
                    "{} already exists; pass --force to overwrite generated federation material",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn new_ceremony_id(
    public_config: &FederationConfig,
    keyset_spec: &FederationDkgKeysetSpec,
) -> Result<FederationDkgCeremonyId> {
    let mut material = b"cdk-federation-production-dkg-ceremony-v1".to_vec();
    material.extend_from_slice(&public_config.federation_id.to_bytes());
    material.extend_from_slice(&serde_json::to_vec(keyset_spec)?);
    material.extend_from_slice(&SecretKey::generate().public_key().to_bytes());
    Ok(FederationDkgCeremonyId::from_bytes(
        sha256::Hash::hash(&material).to_byte_array(),
    ))
}

fn federation_endpoints_are_loopback(public_config: &FederationConfig) -> bool {
    public_config.members.values().all(|member| {
        Url::parse(&member.federation_api_url.to_string())
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback())
            })
    })
}

fn validate_enrollment_url(
    url: &MintUrl,
    label: &str,
    require_secure_transport: bool,
) -> Result<()> {
    let parsed = Url::parse(&url.to_string()).with_context(|| format!("invalid {label} URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("{label} URL must not contain embedded credentials");
    }
    if require_secure_transport && parsed.scheme() != "https" && !url_host_is_loopback(&parsed) {
        bail!("{label} URL must use HTTPS unless its host is loopback");
    }
    Ok(())
}

fn url_host_is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn ceremony_config_toml(
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: &FederationDkgKeysetSpec,
    allow_insecure_localhost: bool,
) -> Result<String> {
    let amounts = keyset_spec
        .amounts
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let expiry = keyset_spec
        .final_expiry
        .map(|expiry| format!("expiry = {expiry}\n"))
        .unwrap_or_default();
    Ok(format!(
        "[federation.dkg]\n\
enabled = true\n\
ceremony_id = {}\n\
unit = {}\n\
active = {}\n\
input_fee_ppk = {}\n\
{}amounts = [{}]\n\
max_ticks = 1200\n\
retry_delay_ms = 250\n\
member_request_burst = 16\n\
member_request_refill_ms = 250\n\
allow_insecure_localhost = {}\n",
        serde_json::to_string(&ceremony_id.to_string())?,
        serde_json::to_string(&keyset_spec.unit.to_string().to_lowercase())?,
        keyset_spec.active,
        keyset_spec.input_fee_ppk,
        expiry,
        amounts,
        allow_insecure_localhost,
    ))
}

#[cfg(test)]
mod tests;
