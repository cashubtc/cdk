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
    generate_trusted_dealer_configs, FederatedKeyset, FederationConfig,
    FederationDkgCommitmentRequest, FederationDkgCommitmentRound,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupMethod,
    FederationDkgTranscript, FederationDkgTranscriptSignature,
    FederationDkgTranscriptSignatureRequest, FederationRestMessageKind, FederationSignedRequest,
    MemberConfigPrivate, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::mint_url::MintUrl;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::nuts::nut01::{evaluate_secret_key_coefficients, BlsSecretKey};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use cdk_common::{Amount, CurrencyUnit, SecretKey};

#[cfg(feature = "federation")]
use crate::cli::{FederationTrustedDealerArgs, FederationTrustedDealerDatabaseEngine};

#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PUBLIC_CONFIG_FILE: &str = "federation-public.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_PUBLIC_CONFIG_FILE: &str = "dkg-setup-public.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const PRIVATE_CONFIG_FILE: &str = "federation-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_PRIVATE_CONFIG_FILE: &str = "dkg-setup-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_FILE: &str = "config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_MEMBER_CONFIG_FILE: &str = "dkg-setup-config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_FILE: &str = "smoke-local-federation.sh";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_ROUTE_PROBE_DIR: &str = "dkg-setup-route-probe";

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerArtifacts {
    pub(crate) public_config_path: PathBuf,
    pub(crate) dkg_setup_public_config_path: PathBuf,
    pub(crate) smoke_script_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerMemberArtifacts {
    pub(crate) config_path: PathBuf,
    pub(crate) dkg_setup_config_path: PathBuf,
    pub(crate) private_config_path: PathBuf,
    pub(crate) dkg_setup_private_config_path: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub(crate) dkg_setup_work_dir: PathBuf,
    pub(crate) public_mint_url: String,
    pub(crate) federation_api_url: String,
    pub(crate) dkg_setup_request_dir: PathBuf,
    pub(crate) prometheus_url: String,
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
        for (member_id, member) in &artifacts.member_artifacts {
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
        println!(
            "Run local federation smoke test: {}",
            artifacts.smoke_script_path.display()
        );

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
        true,
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
    )?;
    let rotation_keyset_spec = trusted_dealer_keyset_spec(
        unit.clone(),
        false,
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
    )?;

    let bundle = generate_trusted_dealer_configs(
        threshold_params,
        members,
        vec![keyset_spec, rotation_keyset_spec],
    )
    .context("failed to generate trusted-dealer federation configs")?;
    validate_bundle(&bundle)?;
    let dkg_setup_public_config = dkg_setup_public_config(&bundle.public_config)?;
    let dkg_setup_request_dirs = write_dkg_setup_route_probe_requests(
        &output_dir,
        &bundle.public_config,
        &bundle.private_configs,
        &unit,
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
        args.force,
    )?;

    let public_config_path = output_dir.join(PUBLIC_CONFIG_FILE);
    write_json_file(&public_config_path, &bundle.public_config, args.force)?;
    let dkg_setup_public_config_path = output_dir.join(DKG_SETUP_PUBLIC_CONFIG_FILE);
    write_json_file(
        &dkg_setup_public_config_path,
        &dkg_setup_public_config,
        args.force,
    )?;

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
        let dkg_setup_private_config =
            dkg_setup_private_config(private_config, &dkg_setup_public_config)?;
        let dkg_setup_private_config_path = member_dir.join(DKG_SETUP_PRIVATE_CONFIG_FILE);
        write_json_file(
            &dkg_setup_private_config_path,
            &dkg_setup_private_config,
            args.force,
        )?;

        let work_dir = member_dir.join("work");
        let dkg_setup_work_dir = member_dir.join("dkg-setup-work");
        let config_path = member_dir.join(MEMBER_CONFIG_FILE);
        let dkg_setup_config_path = member_dir.join(DKG_SETUP_MEMBER_CONFIG_FILE);
        let member_url = member.public_mint_url.to_string();
        let federation_api_url = member.federation_api_url.to_string();
        let database_config = member_database_config(args, *member_id)?;
        let listen_port = args
            .port_start
            .checked_add(member_id.get() - 1)
            .ok_or_else(|| anyhow!("member ports overflow u16"))?;
        let prometheus_port = args
            .port_start
            .checked_add(args.members)
            .and_then(|port| port.checked_add(member_id.get() - 1))
            .ok_or_else(|| anyhow!("member Prometheus ports overflow u16"))?;
        let prometheus_url = format!("http://127.0.0.1:{prometheus_port}/metrics");
        let config_toml = member_config_toml(
            member_id,
            &args.host,
            &member_url,
            &federation_api_url,
            listen_port,
            prometheus_port,
            &public_config_path,
            &private_config_path,
            &config_path,
            &work_dir,
            &unit,
            &database_config,
        )?;
        write_text_file(&config_path, &config_toml, args.force)?;
        let dkg_setup_config_toml = member_config_toml(
            member_id,
            &args.host,
            &member_url,
            &federation_api_url,
            listen_port,
            prometheus_port,
            &dkg_setup_public_config_path,
            &dkg_setup_private_config_path,
            &dkg_setup_config_path,
            &dkg_setup_work_dir,
            &unit,
            &database_config,
        )?;
        write_text_file(&dkg_setup_config_path, &dkg_setup_config_toml, args.force)?;

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                dkg_setup_config_path,
                private_config_path,
                dkg_setup_private_config_path,
                work_dir,
                dkg_setup_work_dir,
                public_mint_url: member_url,
                federation_api_url,
                dkg_setup_request_dir: dkg_setup_request_dirs
                    .get(member_id)
                    .ok_or_else(|| anyhow!("missing DKG route-probe dir for member {member_id}"))?
                    .clone(),
                prometheus_url,
            },
        );
    }

    let smoke_script_path = output_dir.join(SMOKE_SCRIPT_FILE);
    let smoke_script = local_smoke_script(
        &public_config_path,
        &dkg_setup_public_config_path,
        &member_artifacts,
        args,
    )?;
    write_executable_text_file(&smoke_script_path, &smoke_script, args.force)?;

    Ok(TrustedDealerArtifacts {
        public_config_path,
        dkg_setup_public_config_path,
        smoke_script_path,
        member_artifacts,
    })
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_setup_public_config(public_config: &FederationConfig) -> Result<FederationConfig> {
    let mut setup_public_config = public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config
        .validate()
        .context("generated setup-only public federation config is invalid")?;
    Ok(setup_public_config)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_setup_private_config(
    private_config: &MemberConfigPrivate,
    setup_public_config: &FederationConfig,
) -> Result<MemberConfigPrivate> {
    let mut setup_private_config = private_config.clone();
    setup_private_config.keyset_secret_shares.clear();
    setup_private_config.setup_transcript_hash = None;
    setup_private_config
        .validate_against_public(setup_public_config)
        .context("generated setup-only private federation config is invalid")?;
    Ok(setup_private_config)
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
    match args.database_engine {
        FederationTrustedDealerDatabaseEngine::Sqlite => {
            if args.postgres_url_template.is_some() || args.postgres_base_url.is_some() {
                bail!(
                    "--postgres-url-template and --postgres-base-url require --database-engine postgres"
                );
            }
        }
        FederationTrustedDealerDatabaseEngine::Postgres => {
            match (
                args.postgres_url_template.as_deref(),
                args.postgres_base_url.as_deref(),
            ) {
                (Some(_), Some(_)) => {
                    bail!("use either --postgres-url-template or --postgres-base-url, not both");
                }
                (None, None) => {
                    bail!(
                        "--postgres-url-template or --postgres-base-url is required with --database-engine postgres"
                    );
                }
                (Some(template), None) => {
                    if template.trim().is_empty() {
                        bail!("--postgres-url-template must not be empty");
                    }
                    if !template.contains("{member}") {
                        bail!("--postgres-url-template must contain {{member}} so each federation member has distinct Postgres storage");
                    }
                }
                (None, Some(base_url)) => {
                    if base_url.trim().is_empty() {
                        bail!("--postgres-base-url must not be empty");
                    }
                }
            }
        }
    }
    let last_port_offset = args
        .members
        .checked_mul(2)
        .and_then(|ports| ports.checked_sub(1))
        .ok_or_else(|| anyhow!("member and Prometheus port span overflows u16"))?;
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
    active: bool,
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
        active,
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
fn write_dkg_setup_route_probe_requests(
    output_dir: &Path,
    public_config: &FederationConfig,
    private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    unit: &CurrencyUnit,
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    threshold: u16,
    amounts: &[u64],
    force: bool,
) -> Result<BTreeMap<MemberId, PathBuf>> {
    let coefficients = dkg_probe_coefficients(public_config, threshold, amounts)?;
    let reveals = coefficients
        .keys()
        .map(|member_id| {
            Ok((
                *member_id,
                dkg_probe_reveal(public_config, *member_id, &coefficients)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let commitments = reveals
        .iter()
        .map(|(member_id, reveal)| {
            Ok((
                *member_id,
                FederationDkgParticipantCommitment::from_reveal(reveal)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let round = FederationDkgCommitmentRound::new(public_config, commitments.clone())
        .context("generated DKG route-probe commitments are invalid")?;
    round
        .validate_reveals(public_config, &reveals)
        .context("generated DKG route-probe reveals are invalid")?;

    let mut derived_keysets = BTreeMap::new();
    for receiver_id in public_config.members.keys().copied() {
        let mut participant_secret_shares = BTreeMap::new();
        for sender_id in public_config.members.keys().copied() {
            let secret_share_payload = dkg_probe_secret_share_request(
                public_config,
                &coefficients,
                sender_id,
                receiver_id,
            )?;
            participant_secret_shares.insert(sender_id, secret_share_payload.secret_shares);
        }

        let keyset_share = round
            .derive_member_keyset_share(
                public_config,
                &reveals,
                receiver_id,
                &participant_secret_shares,
                unit.clone(),
                true,
                input_fee_ppk,
                final_expiry,
            )
            .with_context(|| {
                format!("generated DKG route-probe shares for member {receiver_id} are invalid")
            })?;
        derived_keysets.insert(receiver_id, keyset_share.keyset);
    }
    let route_probe_keyset = shared_dkg_probe_keyset(&derived_keysets)?;
    let final_public_config = FederationConfig::new(
        public_config.threshold_params,
        public_config.members.clone(),
        vec![route_probe_keyset],
    )
    .context("generated DKG route-probe final public config is invalid")?;
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
    )
    .context("generated DKG route-probe transcript hash is invalid")?;
    let transcript_signature_payloads = private_configs
        .iter()
        .map(|(sender_id, private_config)| {
            Ok((
                *sender_id,
                FederationDkgTranscriptSignatureRequest::new(
                    final_public_config.federation_id,
                    *sender_id,
                    transcript_hash,
                    FederationDkgTranscriptSignature::sign(transcript_hash, private_config)?,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let transcript = FederationDkgTranscript::from_signatures(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        transcript_signature_payloads
            .iter()
            .map(|(member_id, payload)| (*member_id, payload.signature))
            .collect(),
    )
    .context("generated DKG route-probe transcript signatures are invalid")?;
    if transcript.transcript_hash != transcript_hash {
        bail!("generated DKG route-probe transcript hash mismatch");
    }

    let mut request_dirs = BTreeMap::new();
    for receiver_id in public_config.members.keys().copied() {
        let receiver_dir = output_dir
            .join(DKG_SETUP_ROUTE_PROBE_DIR)
            .join(format!("receiver-{}", receiver_id.get()));
        request_dirs.insert(receiver_id, receiver_dir.clone());

        for sender_id in public_config.members.keys().copied() {
            let private_config = private_configs
                .get(&sender_id)
                .ok_or_else(|| anyhow!("missing private config for member {sender_id}"))?;
            let commitment_payload =
                FederationDkgCommitmentRequest::new(*commitments.get(&sender_id).ok_or_else(
                    || anyhow!("missing DKG route-probe commitment for member {sender_id}"),
                )?);
            write_signed_dkg_probe_request(
                public_config,
                private_config,
                receiver_id,
                1,
                FederationRestMessageKind::DkgCommitment,
                commitment_payload,
                &dkg_probe_request_path(&receiver_dir, sender_id, "commitment"),
                force,
            )?;

            let reveal_payload = FederationDkgRevealRequest::new(
                reveals
                    .get(&sender_id)
                    .ok_or_else(|| {
                        anyhow!("missing DKG route-probe reveal for member {sender_id}")
                    })?
                    .clone(),
            );
            write_signed_dkg_probe_request(
                public_config,
                private_config,
                receiver_id,
                2,
                FederationRestMessageKind::DkgReveal,
                reveal_payload,
                &dkg_probe_request_path(&receiver_dir, sender_id, "reveal"),
                force,
            )?;

            let secret_share_payload = dkg_probe_secret_share_request(
                public_config,
                &coefficients,
                sender_id,
                receiver_id,
            )?;
            write_signed_dkg_probe_request(
                public_config,
                private_config,
                receiver_id,
                3,
                FederationRestMessageKind::DkgSecretShare,
                secret_share_payload,
                &dkg_probe_request_path(&receiver_dir, sender_id, "secret-share"),
                force,
            )?;

            let transcript_signature_payload = *transcript_signature_payloads
                .get(&sender_id)
                .ok_or_else(|| {
                    anyhow!("missing DKG route-probe transcript signature for member {sender_id}")
                })?;
            write_signed_dkg_probe_request(
                public_config,
                private_config,
                receiver_id,
                4,
                FederationRestMessageKind::DkgTranscriptSignature,
                transcript_signature_payload,
                &dkg_probe_request_path(&receiver_dir, sender_id, "transcript-signature"),
                force,
            )?;
        }
    }

    Ok(request_dirs)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn shared_dkg_probe_keyset(
    derived_keysets: &BTreeMap<MemberId, FederatedKeyset>,
) -> Result<FederatedKeyset> {
    let first = derived_keysets
        .values()
        .next()
        .ok_or_else(|| anyhow!("generated DKG route-probe has no derived keysets"))?;
    for (member_id, keyset) in derived_keysets {
        if keyset != first {
            bail!("generated DKG route-probe keyset for member {member_id} differs");
        }
    }
    Ok(first.clone())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_probe_coefficients(
    public_config: &FederationConfig,
    threshold: u16,
    amounts: &[u64],
) -> Result<BTreeMap<MemberId, BTreeMap<Amount, Vec<BlsSecretKey>>>> {
    let mut coefficients = BTreeMap::new();
    for member_id in public_config.members.keys().copied() {
        let mut member_coefficients = BTreeMap::new();
        for amount in amounts {
            if member_coefficients
                .insert(Amount::from(*amount), random_bls_coefficients(threshold)?)
                .is_some()
            {
                bail!("duplicate federation amount {amount}");
            }
        }
        coefficients.insert(member_id, member_coefficients);
    }
    Ok(coefficients)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_probe_reveal(
    public_config: &FederationConfig,
    member_id: MemberId,
    coefficients: &BTreeMap<MemberId, BTreeMap<Amount, Vec<BlsSecretKey>>>,
) -> Result<FederationDkgParticipantCommitmentReveal> {
    let member_coefficients = coefficients
        .get(&member_id)
        .ok_or_else(|| anyhow!("missing DKG route-probe coefficients for member {member_id}"))?;
    let coefficient_commitments = member_coefficients
        .iter()
        .map(|(amount, coefficients)| {
            (
                *amount,
                coefficients
                    .iter()
                    .map(BlsSecretKey::public_key_g2)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    Ok(FederationDkgParticipantCommitmentReveal {
        federation_id: public_config.federation_id,
        member_id,
        coefficient_commitments,
    })
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_probe_secret_share_request(
    public_config: &FederationConfig,
    coefficients: &BTreeMap<MemberId, BTreeMap<Amount, Vec<BlsSecretKey>>>,
    sender_id: MemberId,
    receiver_id: MemberId,
) -> Result<FederationDkgSecretShareRequest> {
    let sender_coefficients = coefficients
        .get(&sender_id)
        .ok_or_else(|| anyhow!("missing DKG route-probe coefficients for member {sender_id}"))?;
    let receiver_signer_id = receiver_id.to_bls_signer_id();
    let secret_shares = sender_coefficients
        .iter()
        .map(|(amount, coefficients)| {
            Ok((
                *amount,
                evaluate_secret_key_coefficients(receiver_signer_id, coefficients)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok(FederationDkgSecretShareRequest::new(
        public_config.federation_id,
        sender_id,
        receiver_id,
        secret_shares,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_signed_dkg_probe_request<T>(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver_id: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    payload: T,
    path: &Path,
    force: bool,
) -> Result<()>
where
    T: serde::Serialize,
{
    let request = FederationSignedRequest::new_signed(
        public_config.federation_id,
        receiver_id,
        sequence,
        kind,
        payload,
        private_config,
    )?;
    request.verify(public_config, receiver_id)?;
    write_json_file(path, &request, force)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn dkg_probe_request_path(receiver_dir: &Path, sender_id: MemberId, label: &str) -> PathBuf {
    receiver_dir.join(format!("from-{}-{label}.json", sender_id.get()))
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
    prometheus_port: u16,
    public_config_path: &Path,
    private_config_path: &Path,
    config_path: &Path,
    work_dir: &Path,
    unit: &CurrencyUnit,
    database_config: &str,
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

[prometheus]
enabled = true
address = "127.0.0.1"
port = {prometheus_port}

{database_config}
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
        prometheus_port = prometheus_port,
        unit = toml_string(&unit)?,
        database_config = database_config,
        public_config_path = toml_string(&public_config_path)?,
        private_config_path = toml_string(&private_config_path)?,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_database_config(
    args: &FederationTrustedDealerArgs,
    member_id: MemberId,
) -> Result<String> {
    match args.database_engine {
        FederationTrustedDealerDatabaseEngine::Sqlite => {
            Ok("[database]\nengine = \"sqlite\"\n".to_string())
        }
        FederationTrustedDealerDatabaseEngine::Postgres => {
            let member_id = member_id.get().to_string();
            let url = match (
                args.postgres_url_template.as_deref(),
                args.postgres_base_url.as_deref(),
            ) {
                (Some(template), None) => template.replace("{member}", &member_id),
                (None, Some(base_url)) => format!(
                    "{} schema=cdk_federation_member_{}",
                    base_url.trim(),
                    member_id
                ),
                _ => {
                    bail!(
                        "use exactly one of --postgres-url-template or --postgres-base-url with --database-engine postgres"
                    );
                }
            };
            Ok(format!(
                r#"[database]
engine = "postgres"

[database.postgres]
url = {url}
tls_mode = "disable"
max_connections = 20
connection_timeout_seconds = 10
"#,
                url = toml_string(&url)?,
            ))
        }
    }
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

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn write_executable_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
    write_text_file(path, contents, force)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let permissions = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("failed to mark {} executable", path.display()))?;
    }

    Ok(())
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn local_smoke_script(
    public_config_path: &Path,
    dkg_setup_public_config_path: &Path,
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
    args: &FederationTrustedDealerArgs,
) -> Result<String> {
    let members = member_artifacts
        .keys()
        .map(|member_id| member_id.get().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let primary_member = member_artifacts
        .keys()
        .next()
        .ok_or_else(|| anyhow!("local federation smoke script needs at least one member"))?;
    let restart_member = member_artifacts.keys().nth(1).unwrap_or(primary_member);

    let config_cases = member_path_cases(member_artifacts, |member| &member.config_path)?;
    let dkg_setup_config_cases =
        member_path_cases(member_artifacts, |member| &member.dkg_setup_config_path)?;
    let work_dir_cases = member_path_cases(member_artifacts, |member| &member.work_dir)?;
    let dkg_setup_work_dir_cases =
        member_path_cases(member_artifacts, |member| &member.dkg_setup_work_dir)?;
    let private_config_cases =
        member_path_cases(member_artifacts, |member| &member.private_config_path)?;
    let dkg_setup_private_config_cases = member_path_cases(member_artifacts, |member| {
        &member.dkg_setup_private_config_path
    })?;
    let dkg_setup_request_dir_cases =
        member_path_cases(member_artifacts, |member| &member.dkg_setup_request_dir)?;
    let public_mint_url_cases =
        member_string_cases(member_artifacts, |member| &member.public_mint_url);
    let federation_url_cases =
        member_string_cases(member_artifacts, |member| &member.federation_api_url);
    let prometheus_url_cases =
        member_string_cases(member_artifacts, |member| &member.prometheus_url);
    let public_config_path = shell_single_quote(&path_to_string(public_config_path));
    let dkg_setup_public_config_path =
        shell_single_quote(&path_to_string(dkg_setup_public_config_path));
    let postgres_schema_reset_url = postgres_schema_reset_url(args)
        .map(|url| shell_single_quote(&url))
        .unwrap_or_else(|| "''".to_string());
    let postgres_schema_reset_cases = postgres_schema_reset_cases(member_artifacts);

    Ok(format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

CDK_MINTD_BIN="${{CDK_MINTD_BIN:-cdk-mintd}}"
PUBLIC_CONFIG={public_config_path}
DKG_SETUP_PUBLIC_CONFIG={dkg_setup_public_config_path}
POSTGRES_SCHEMA_RESET_URL={postgres_schema_reset_url}
RESET_POSTGRES_SCHEMAS="${{RESET_POSTGRES_SCHEMAS:-0}}"
MEMBERS=({members})
PRIMARY_MEMBER="${{PRIMARY_MEMBER:-{primary_member}}}"
RESTART_MEMBER="${{RESTART_MEMBER:-{restart_member}}}"
QUARANTINE_OFFLINE_MEMBER="${{QUARANTINE_OFFLINE_MEMBER:-$RESTART_MEMBER}}"
STATUS_RETRIES="${{STATUS_RETRIES:-60}}"
STATUS_SLEEP_SECONDS="${{STATUS_SLEEP_SECONDS:-1}}"
CHECK_PROMETHEUS_METRICS="${{CHECK_PROMETHEUS_METRICS:-1}}"
PROBE_DKG_SETUP_ROUTES="${{PROBE_DKG_SETUP_ROUTES:-0}}"
DKG_ROUTE_PROBE_ONLY="${{DKG_ROUTE_PROBE_ONLY:-0}}"
if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
  PROBE_DKG_SETUP_ROUTES=1
  PUBLIC_CONFIG="$DKG_SETUP_PUBLIC_CONFIG"
fi
SMOKE_QUOTE_RETRIES="${{SMOKE_QUOTE_RETRIES:-300}}"
SMOKE_QUOTE_SLEEP_MS="${{SMOKE_QUOTE_SLEEP_MS:-100}}"
OFFLINE_OPERATION_BATCHES="${{OFFLINE_OPERATION_BATCHES:-3}}"
PENDING_WALLET_DB="${{PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/pending-melt-wallet.sqlite}}"
FAILED_PENDING_WALLET_DB="${{FAILED_PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/failed-pending-melt-wallet.sqlite}}"
PIDS=()

member_config() {{
  if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
    case "$1" in
{dkg_setup_config_cases}      *) return 1 ;;
    esac
    return 0
  fi
  case "$1" in
{config_cases}    *) return 1 ;;
  esac
}}

member_work_dir() {{
  if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
    case "$1" in
{dkg_setup_work_dir_cases}      *) return 1 ;;
    esac
    return 0
  fi
  case "$1" in
{work_dir_cases}    *) return 1 ;;
  esac
}}

member_private_config() {{
  if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
    case "$1" in
{dkg_setup_private_config_cases}      *) return 1 ;;
    esac
    return 0
  fi
  case "$1" in
{private_config_cases}    *) return 1 ;;
  esac
}}

member_dkg_setup_request_dir() {{
  case "$1" in
{dkg_setup_request_dir_cases}    *) return 1 ;;
  esac
}}

member_mint_url() {{
  case "$1" in
{public_mint_url_cases}    *) return 1 ;;
  esac
}}

member_federation_url() {{
  case "$1" in
{federation_url_cases}    *) return 1 ;;
  esac
}}

member_prometheus_url() {{
  case "$1" in
{prometheus_url_cases}    *) return 1 ;;
  esac
}}

start_member() {{
  local member="$1"
  local config
  local work_dir
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  mkdir -p "$work_dir"
  if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
    CDK_FEDERATION_DISABLE_PEER_SYNC=1 "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 &
  else
    "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 &
  fi
  PIDS[$member]=$!
  printf 'started member %s with pid %s\n' "$member" "${{PIDS[$member]}}"
}}

stop_member() {{
  local member="$1"
  local pid="${{PIDS[$member]:-}}"
  if [[ -z "$pid" ]]; then
    return 0
  fi
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  unset 'PIDS[$member]'
  printf 'stopped member %s\n' "$member"
}}

cleanup() {{
  for member in "${{MEMBERS[@]}}"; do
    stop_member "$member"
  done
}}

postgres_member_schema() {{
  case "$1" in
{postgres_schema_reset_cases}    *) return 1 ;;
  esac
}}

reset_postgres_schemas() {{
  if [[ -z "$POSTGRES_SCHEMA_RESET_URL" ]]; then
    return 0
  fi
  if [[ "$RESET_POSTGRES_SCHEMAS" != "1" ]]; then
    printf 'generated PostgreSQL member schemas are configured; set RESET_POSTGRES_SCHEMAS=1 to reset them before smoke\n' >&2
    return 1
  fi
  if ! command -v psql >/dev/null 2>&1; then
    printf 'psql is required to reset PostgreSQL member schemas before smoke\n' >&2
    return 1
  fi
  local member
  for member in "${{MEMBERS[@]}}"; do
    local schema
    schema="$(postgres_member_schema "$member")"
    psql "$POSTGRES_SCHEMA_RESET_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA IF EXISTS \"$schema\" CASCADE; CREATE SCHEMA \"$schema\";" >/dev/null
  done
}}

reset_member_work_dirs() {{
  rm -f "$PENDING_WALLET_DB" "$PENDING_WALLET_DB-shm" "$PENDING_WALLET_DB-wal"
  rm -f "$FAILED_PENDING_WALLET_DB" "$FAILED_PENDING_WALLET_DB-shm" "$FAILED_PENDING_WALLET_DB-wal"
  reset_postgres_schemas
  local member
  for member in "${{MEMBERS[@]}}"; do
    local work_dir
    work_dir="$(member_work_dir "$member")"
    rm -rf "$work_dir"
    mkdir -p "$work_dir"
  done
}}

wait_for_status() {{
  local member="$1"
  local work_dir
  work_dir="$(member_work_dir "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_status_line "$member" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not answer signed federation status\n' "$member" >&2
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

member_status_line() {{
  local member="$1"
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready | awk '/^member / {{ print; exit }}'
}}

member_dkg_status() {{
  local member="$1"
  local private_config
  local config
  local work_dir
  private_config="$(member_private_config "$member")"
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" federation dkg-status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --from-database --require-ready | awk '/^finalized_ready=true/ {{ print; exit }}'
}}

member_dkg_setup_records_line() {{
  local member="$1"
  local private_config
  local config
  local work_dir
  private_config="$(member_private_config "$member")"
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" federation dkg-status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --from-database | awk '/^setup_records / {{ print; exit }}'
}}

member_dkg_completion_records_line() {{
  local member="$1"
  local private_config
  local config
  local work_dir
  private_config="$(member_private_config "$member")"
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" federation dkg-status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --from-database | awk '/^setup_transcript=/ {{ print; exit }}'
}}

print_dkg_record_runbook() {{
  local member="$1"
  local private_config
  local config
  local work_dir
  private_config="$(member_private_config "$member")"
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  cat >&2 <<RUNBOOK
DKG record recovery for member $member:
  Database-backed readiness:
    "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" federation dkg-status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --from-database --require-ready
  In-progress setup diagnostics:
    The database-backed command prints setup_records with collected commitment, reveal, and secret-share member sets before finalization is ready.
  File-based readiness after exchanging completion records:
    "$CDK_MINTD_BIN" federation dkg-status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --keyset-share <keyset-share.json> --transcript-signature <transcript-signature.json> --require-ready
  Combine --from-database with repeated --keyset-share and --transcript-signature flags while migrating records between members.
RUNBOOK
}}

wait_for_dkg_status() {{
  local member="$1"
  local work_dir
  work_dir="$(member_work_dir "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_dkg_status "$member" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not report usable federation DKG/config material\n' "$member" >&2
  member_dkg_status "$member" >&2 || true
  print_dkg_record_runbook "$member"
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

member_quorum_status_line() {{
  local member="$1"
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready --require-session-outcome-quorum --require-no-digest-alerts | awk '/^member / {{ print; exit }}'
}}

member_payment_report_lines() {{
  local member="$1"
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation payment-observations --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --context mint-quote | awk '/^  payment_report / {{ print }}'
}}

compare_digests() {{
  local signer="${{MEMBERS[0]}}"
  local private_config
  private_config="$(member_private_config "$signer")"
  "$CDK_MINTD_BIN" federation compare-digests --public-config "$PUBLIC_CONFIG" --private-config "$private_config"
}}

wait_for_prometheus_metrics() {{
  if [[ "$CHECK_PROMETHEUS_METRICS" != "1" ]]; then
    return 0
  fi
  if ! command -v curl >/dev/null 2>&1; then
    printf 'curl is required when CHECK_PROMETHEUS_METRICS=1\n' >&2
    return 1
  fi
  local member
  for member in "${{MEMBERS[@]}}"; do
    local metrics_url
    metrics_url="$(member_prometheus_url "$member")"
    local metrics=""
    for _ in $(seq 1 "$STATUS_RETRIES"); do
      if metrics="$(curl -fsS "$metrics_url" 2>/dev/null)" && prometheus_metrics_are_ready "$metrics"; then
        break
      fi
      sleep "$STATUS_SLEEP_SECONDS"
    done
    if ! prometheus_metrics_are_ready "$metrics"; then
      printf 'member %s did not expose federation Prometheus metrics at %s\n' "$member" "$metrics_url" >&2
      return 1
    fi
  done
}}

prometheus_metrics_are_ready() {{
  local metrics="$1"
  local metric
  for metric in \
    cdk_federation_consensus_operation_count \
    cdk_federation_sync_ready \
    cdk_federation_sync_state \
    cdk_federation_session_outcome_quorum_operation_count \
    cdk_federation_payment_observation_reports; do
    if ! grep -q "^$metric" <<<"$metrics"; then
      return 1
    fi
  done
  local federation_metrics
  federation_metrics="$(grep '^cdk_federation_' <<<"$metrics" || true)"
  if grep -Eq '(quote|invoice|proof|secret|path)=' <<<"$federation_metrics"; then
    printf 'federation Prometheus metrics include a sensitive label\n' >&2
    return 1
  fi
  return 0
}}

status_field() {{
  local line="$1"
  local key="$2"
  local field
  for field in $line; do
    case "$field" in
      "$key"=*)
        printf '%s\n' "${{field#*=}}"
        return 0
        ;;
    esac
  done
  return 1
}}

run_smoke_operations() {{
  local has_private_config=0
  local arg
  for arg in "$@"; do
    if [[ "$arg" == "--private-config" ]]; then
      has_private_config=1
      break
    fi
  done
  if [[ "$has_private_config" == "0" ]]; then
    local signer="${{MEMBERS[0]}}"
    "$CDK_MINTD_BIN" federation smoke-operations \
      --public-config "$PUBLIC_CONFIG" \
      --private-config "$(member_private_config "$signer")" \
      --quote-retries "$SMOKE_QUOTE_RETRIES" \
      --quote-sleep-ms "$SMOKE_QUOTE_SLEEP_MS" \
      "$@"
    return
  fi
  "$CDK_MINTD_BIN" federation smoke-operations \
    --public-config "$PUBLIC_CONFIG" \
    --quote-retries "$SMOKE_QUOTE_RETRIES" \
    --quote-sleep-ms "$SMOKE_QUOTE_SLEEP_MS" \
    "$@"
}}

post_dkg_setup_request() {{
  local endpoint="$1"
  local request_file="$2"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if curl -fsS -H 'content-type: application/json' --data-binary "@$request_file" "$endpoint" >/dev/null; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'could not post DKG setup route-probe request %s to %s\n' "$request_file" "$endpoint" >&2
  return 1
}}

probe_dkg_setup_routes() {{
  if [[ "$PROBE_DKG_SETUP_ROUTES" != "1" ]]; then
    return 0
  fi
  if ! command -v curl >/dev/null 2>&1; then
    printf 'curl is required when PROBE_DKG_SETUP_ROUTES=1\n' >&2
    return 1
  fi
  local receiver
  for receiver in "${{MEMBERS[@]}}"; do
    local receiver_url
    local request_dir
    receiver_url="$(member_federation_url "$receiver")"
    request_dir="$(member_dkg_setup_request_dir "$receiver")"
    local sender
    for sender in "${{MEMBERS[@]}}"; do
      post_dkg_setup_request "$receiver_url/federation/v1/dkg/commitments" "$request_dir/from-$sender-commitment.json"
    done
    for sender in "${{MEMBERS[@]}}"; do
      post_dkg_setup_request "$receiver_url/federation/v1/dkg/reveals" "$request_dir/from-$sender-reveal.json"
    done
    for sender in "${{MEMBERS[@]}}"; do
      post_dkg_setup_request "$receiver_url/federation/v1/dkg/secret-shares" "$request_dir/from-$sender-secret-share.json"
    done
    for sender in "${{MEMBERS[@]}}"; do
      post_dkg_setup_request "$receiver_url/federation/v1/dkg/transcript-signatures" "$request_dir/from-$sender-transcript-signature.json"
    done
  done
}}

wait_for_converged() {{
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    local baseline_operations=""
    local baseline_order_digest=""
    local baseline_state_digest=""
    local converged=1
    local member
    for member in "${{MEMBERS[@]}}"; do
      local line
      local operations
      local order_digest
      local state_digest
      if ! line="$(member_status_line "$member" 2>/dev/null)"; then
        converged=0
        break
      fi
      if ! operations="$(status_field "$line" operations)"; then
        converged=0
        break
      fi
      if ! order_digest="$(status_field "$line" order_digest)"; then
        converged=0
        break
      fi
      if ! state_digest="$(status_field "$line" state_digest)"; then
        converged=0
        break
      fi
      if [[ -z "$baseline_operations" ]]; then
        baseline_operations="$operations"
        baseline_order_digest="$order_digest"
        baseline_state_digest="$state_digest"
        continue
      fi
      if [[ "$operations" != "$baseline_operations" || "$order_digest" != "$baseline_order_digest" || "$state_digest" != "$baseline_state_digest" ]]; then
        converged=0
        break
      fi
    done
    if [[ "$converged" == "1" ]]; then
      if compare_digests >/dev/null 2>&1; then
        return 0
      fi
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'federation did not converge\n' >&2
  for member in "${{MEMBERS[@]}}"; do
    member_status_line "$member" >&2 || true
  done
  compare_digests >&2 || true
  return 1
}}

wait_for_dkg_setup_records() {{
  if [[ "$PROBE_DKG_SETUP_ROUTES" != "1" ]]; then
    return 0
  fi
  local expected="${{#MEMBERS[@]}}"
  local expected_keyset_shares="1"
  local member
  for member in "${{MEMBERS[@]}}"; do
    local ready=0
    for _ in $(seq 1 "$STATUS_RETRIES"); do
      local line
      local completion_line
      if line="$(member_dkg_setup_records_line "$member" 2>/dev/null)" && completion_line="$(member_dkg_completion_records_line "$member" 2>/dev/null)"; then
        local commitments
        local reveals
        local secret_shares
        local keyset_shares
        local transcript_signatures
        commitments="$(status_field "$line" commitments || true)"
        reveals="$(status_field "$line" reveals || true)"
        secret_shares="$(status_field "$line" secret_share_contributions || true)"
        keyset_shares="$(status_field "$completion_line" keyset_share_records || true)"
        transcript_signatures="$(status_field "$completion_line" transcript_signature_records || true)"
        if [[ "$commitments" == "$expected" && "$reveals" == "$expected" && "$secret_shares" == "$expected" && "$keyset_shares" == "$expected_keyset_shares" && "$transcript_signatures" == "$expected" ]]; then
          ready=1
          break
        fi
      fi
      sleep "$STATUS_SLEEP_SECONDS"
    done
    if [[ "$ready" == "1" ]]; then
      continue
    fi
    printf 'member %s did not persist DKG setup route-probe records\n' "$member" >&2
    member_dkg_setup_records_line "$member" >&2 || true
    member_dkg_completion_records_line "$member" >&2 || true
    return 1
  done
  return 0
}}

wait_for_runtime_keysets() {{
  local member="$1"
  local work_dir
  local mint_url
  work_dir="$(member_work_dir "$member")"
  mint_url="$(member_mint_url "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if curl -fsS "$mint_url/v1/keysets" 2>/dev/null | grep -q '"id"'; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not expose activated DKG keysets after restart\n' "$member" >&2
  curl -fsS "$mint_url/v1/keysets" >&2 || true
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

wait_for_session_outcome_quorum() {{
  local member="$1"
  local work_dir
  work_dir="$(member_work_dir "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_quorum_status_line "$member" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not report quorum-backed session outcome evidence\n' "$member" >&2
  member_quorum_status_line "$member" >&2 || true
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

wait_for_quarantine_report() {{
  local member="$1"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    if member_payment_report_lines "$member" | grep -q 'quarantined=true'; then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not report an accepted payment observation quarantine\n' "$member" >&2
  local private_config
  private_config="$(member_private_config "$member")"
  "$CDK_MINTD_BIN" federation payment-observations --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --context mint-quote >&2 || true
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready >&2 || true
  return 1
}}

validate_quarantine_members() {{
  member_config "$PRIMARY_MEMBER" >/dev/null || {{
    printf 'unknown PRIMARY_MEMBER %s\n' "$PRIMARY_MEMBER" >&2
    return 1
  }}
  member_config "$RESTART_MEMBER" >/dev/null || {{
    printf 'unknown RESTART_MEMBER %s\n' "$RESTART_MEMBER" >&2
    return 1
  }}
  if [[ "$PRIMARY_MEMBER" == "$RESTART_MEMBER" ]]; then
    printf 'RESTART_MEMBER must differ from PRIMARY_MEMBER for offline operation smoke\n' >&2
    return 1
  fi
  member_config "$QUARANTINE_OFFLINE_MEMBER" >/dev/null || {{
    printf 'unknown QUARANTINE_OFFLINE_MEMBER %s\n' "$QUARANTINE_OFFLINE_MEMBER" >&2
    return 1
  }}
  if [[ "$PRIMARY_MEMBER" == "$QUARANTINE_OFFLINE_MEMBER" ]]; then
    printf 'QUARANTINE_OFFLINE_MEMBER must differ from PRIMARY_MEMBER for offline catch-up smoke\n' >&2
    return 1
  fi
}}

validate_smoke_settings() {{
  if ! [[ "$OFFLINE_OPERATION_BATCHES" =~ ^[1-9][0-9]*$ ]]; then
    printf 'OFFLINE_OPERATION_BATCHES must be a positive integer\n' >&2
    return 1
  fi
  if (( OFFLINE_OPERATION_BATCHES > 255 )); then
    printf 'OFFLINE_OPERATION_BATCHES must be 255 or lower\n' >&2
    return 1
  fi
}}

trap cleanup EXIT INT TERM

validate_quarantine_members
validate_smoke_settings
reset_member_work_dirs

for member in "${{MEMBERS[@]}}"; do
  start_member "$member"
done

probe_dkg_setup_routes
wait_for_dkg_setup_records
if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
  for member in "${{MEMBERS[@]}}"; do
    wait_for_dkg_status "$member"
  done
  for member in "${{MEMBERS[@]}}"; do
    stop_member "$member"
  done
  for member in "${{MEMBERS[@]}}"; do
    start_member "$member"
  done
  for member in "${{MEMBERS[@]}}"; do
    wait_for_runtime_keysets "$member"
  done
  printf 'dkg setup route probe passed\n'
  exit 0
fi

for member in "${{MEMBERS[@]}}"; do
  wait_for_status "$member"
  wait_for_dkg_status "$member"
done

wait_for_converged
wait_for_prometheus_metrics
run_smoke_operations
wait_for_converged

stop_member "$RESTART_MEMBER"
for batch in $(seq 1 "$OFFLINE_OPERATION_BATCHES"); do
  run_smoke_operations --scenario offline-member-operations --seed-offset "$batch"
done
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"
wait_for_converged
wait_for_session_outcome_quorum "$RESTART_MEMBER"

run_smoke_operations --scenario pending-melt-start --wallet-db "$PENDING_WALLET_DB"
wait_for_converged

stop_member "$RESTART_MEMBER"
run_smoke_operations --private-config "$(member_private_config "$PRIMARY_MEMBER")" --scenario pending-melt-finish --wallet-db "$PENDING_WALLET_DB"
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"
wait_for_converged
wait_for_session_outcome_quorum "$RESTART_MEMBER"

run_smoke_operations --scenario pending-melt-fail-start --wallet-db "$FAILED_PENDING_WALLET_DB"
wait_for_converged

stop_member "$RESTART_MEMBER"
run_smoke_operations --private-config "$(member_private_config "$PRIMARY_MEMBER")" --scenario pending-melt-fail-finish --wallet-db "$FAILED_PENDING_WALLET_DB"
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"
wait_for_converged
wait_for_session_outcome_quorum "$RESTART_MEMBER"

stop_member "$QUARANTINE_OFFLINE_MEMBER"
run_smoke_operations --private-config "$(member_private_config "$PRIMARY_MEMBER")" --scenario quarantine-mint-conflict
wait_for_quarantine_report "$PRIMARY_MEMBER"

start_member "$QUARANTINE_OFFLINE_MEMBER"
wait_for_status "$QUARANTINE_OFFLINE_MEMBER"

wait_for_converged
wait_for_session_outcome_quorum "$QUARANTINE_OFFLINE_MEMBER"
wait_for_quarantine_report "$QUARANTINE_OFFLINE_MEMBER"

stop_member "$RESTART_MEMBER"
run_smoke_operations --private-config "$(member_private_config "$PRIMARY_MEMBER")" --scenario offline-member-keyset-rotation
start_member "$RESTART_MEMBER"
wait_for_status "$RESTART_MEMBER"
wait_for_converged
wait_for_session_outcome_quorum "$RESTART_MEMBER"
wait_for_prometheus_metrics

printf 'local federation smoke test passed\n'
"#,
        public_config_path = public_config_path,
        dkg_setup_public_config_path = dkg_setup_public_config_path,
        members = members,
        primary_member = primary_member.get(),
        restart_member = restart_member.get(),
        config_cases = config_cases,
        dkg_setup_config_cases = dkg_setup_config_cases,
        work_dir_cases = work_dir_cases,
        dkg_setup_work_dir_cases = dkg_setup_work_dir_cases,
        private_config_cases = private_config_cases,
        dkg_setup_private_config_cases = dkg_setup_private_config_cases,
        dkg_setup_request_dir_cases = dkg_setup_request_dir_cases,
        public_mint_url_cases = public_mint_url_cases,
        federation_url_cases = federation_url_cases,
        prometheus_url_cases = prometheus_url_cases,
        postgres_schema_reset_url = postgres_schema_reset_url,
        postgres_schema_reset_cases = postgres_schema_reset_cases,
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn postgres_schema_reset_url(args: &FederationTrustedDealerArgs) -> Option<String> {
    match args.database_engine {
        FederationTrustedDealerDatabaseEngine::Sqlite => None,
        FederationTrustedDealerDatabaseEngine::Postgres => args
            .postgres_base_url
            .as_deref()
            .map(|url| url.trim().to_string()),
    }
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn postgres_schema_reset_cases(
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
) -> String {
    let mut cases = String::new();
    for member_id in member_artifacts.keys() {
        let schema = format!("cdk_federation_member_{}", member_id.get());
        cases.push_str(&format!(
            "    {}) printf '%s\\n' {} ;;\n",
            member_id.get(),
            shell_single_quote(&schema)
        ));
    }
    cases
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_path_cases<F>(
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
    path: F,
) -> Result<String>
where
    F: Fn(&TrustedDealerMemberArtifacts) -> &Path,
{
    let mut cases = String::new();
    for (member_id, member) in member_artifacts {
        cases.push_str(&format!(
            "    {}) printf '%s\\n' {} ;;\n",
            member_id.get(),
            shell_single_quote(&path_to_string(path(member)))
        ));
    }
    Ok(cases)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn member_string_cases<F>(
    member_artifacts: &BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
    value: F,
) -> String
where
    F: Fn(&TrustedDealerMemberArtifacts) -> &str,
{
    let mut cases = String::new();
    for (member_id, member) in member_artifacts {
        cases.push_str(&format!(
            "    {}) printf '%s\\n' {} ;;\n",
            member_id.get(),
            shell_single_quote(value(member))
        ));
    }
    cases
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
            database_engine: FederationTrustedDealerDatabaseEngine::Sqlite,
            postgres_url_template: None,
            postgres_base_url: None,
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
        assert_eq!(public_config.keysets.len(), 2);
        assert_eq!(
            public_config
                .keysets
                .iter()
                .filter(|keyset| keyset.active)
                .count(),
            1
        );
        assert_eq!(
            public_config
                .keysets
                .iter()
                .filter(|keyset| !keyset.active)
                .count(),
            1
        );
        let setup_transcript = public_config
            .setup_transcript
            .as_ref()
            .expect("setup transcript");
        let dkg_setup_public_json =
            std::fs::read_to_string(&artifacts.dkg_setup_public_config_path)
                .expect("setup-only public config file");
        let dkg_setup_public_config: FederationConfig =
            serde_json::from_str(&dkg_setup_public_json).expect("setup-only public config json");
        dkg_setup_public_config
            .validate()
            .expect("setup-only public config validates");
        assert_eq!(
            dkg_setup_public_config.federation_id,
            public_config.federation_id
        );
        assert!(dkg_setup_public_config.keysets.is_empty());
        assert!(dkg_setup_public_config.setup_transcript.is_none());
        let first_member = MemberId::new(1).expect("first member id");
        let second_member = MemberId::new(2).expect("second member id");
        let first_receiver_probe_dir = output_dir
            .join(DKG_SETUP_ROUTE_PROBE_DIR)
            .join("receiver-1");
        let commitment_json =
            std::fs::read_to_string(first_receiver_probe_dir.join("from-1-commitment.json"))
                .expect("DKG commitment route-probe request");
        let commitment_request: FederationSignedRequest<FederationDkgCommitmentRequest> =
            serde_json::from_str(&commitment_json).expect("signed DKG commitment request");
        commitment_request
            .verify(&public_config, first_member)
            .expect("commitment request verifies");
        commitment_request
            .payload
            .validate_sender(&public_config, first_member)
            .expect("commitment sender validates");
        assert_eq!(commitment_request.sequence, 1);
        assert_eq!(
            commitment_request.kind,
            FederationRestMessageKind::DkgCommitment
        );
        let reveal_json =
            std::fs::read_to_string(first_receiver_probe_dir.join("from-1-reveal.json"))
                .expect("DKG reveal route-probe request");
        let reveal_request: FederationSignedRequest<FederationDkgRevealRequest> =
            serde_json::from_str(&reveal_json).expect("signed DKG reveal request");
        reveal_request
            .verify(&public_config, first_member)
            .expect("reveal request verifies");
        reveal_request
            .payload
            .validate_sender(&public_config, first_member)
            .expect("reveal sender validates");
        assert_eq!(reveal_request.sequence, 2);
        assert_eq!(reveal_request.kind, FederationRestMessageKind::DkgReveal);
        let secret_share_json =
            std::fs::read_to_string(first_receiver_probe_dir.join("from-2-secret-share.json"))
                .expect("DKG secret-share route-probe request");
        let secret_share_request: FederationSignedRequest<FederationDkgSecretShareRequest> =
            serde_json::from_str(&secret_share_json).expect("signed DKG secret-share request");
        secret_share_request
            .verify(&public_config, first_member)
            .expect("secret-share request verifies");
        secret_share_request
            .payload
            .validate_sender_receiver(&public_config, second_member, first_member)
            .expect("secret-share sender and receiver validate");
        assert_eq!(secret_share_request.sequence, 3);
        assert_eq!(
            secret_share_request.kind,
            FederationRestMessageKind::DkgSecretShare
        );
        let transcript_signature_json = std::fs::read_to_string(
            first_receiver_probe_dir.join("from-2-transcript-signature.json"),
        )
        .expect("DKG transcript-signature route-probe request");
        let transcript_signature_request: FederationSignedRequest<
            FederationDkgTranscriptSignatureRequest,
        > = serde_json::from_str(&transcript_signature_json)
            .expect("signed DKG transcript-signature request");
        transcript_signature_request
            .verify(&public_config, first_member)
            .expect("transcript-signature request verifies");
        transcript_signature_request
            .payload
            .validate_sender(&public_config, second_member)
            .expect("transcript-signature sender validates");
        assert_eq!(transcript_signature_request.sequence, 4);
        assert_eq!(
            transcript_signature_request.kind,
            FederationRestMessageKind::DkgTranscriptSignature
        );

        let smoke_script =
            std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
        assert!(smoke_script.starts_with("#!/usr/bin/env bash"));
        assert!(smoke_script.contains("federation status"));
        assert!(smoke_script.contains("federation dkg-status"));
        assert!(smoke_script.contains("federation payment-observations"));
        assert!(smoke_script.contains("federation smoke-operations"));
        assert!(smoke_script.contains("--require-ready"));
        assert!(smoke_script.contains("--from-database"));
        assert!(smoke_script.contains("member_status_line()"));
        assert!(smoke_script.contains("member_dkg_status()"));
        assert!(smoke_script.contains("compare_digests()"));
        assert!(smoke_script.contains("local signer=\"${MEMBERS[0]}\""));
        assert!(smoke_script.contains("private_config=\"$(member_private_config \"$signer\")\""));
        assert!(smoke_script.contains("federation compare-digests"));
        assert!(smoke_script.contains("compare_digests >/dev/null 2>&1"));
        assert!(smoke_script.contains("compare_digests >&2 || true"));
        assert!(smoke_script
            .contains("--work-dir \"$work_dir\" --config \"$config\" federation dkg-status"));
        assert!(smoke_script.contains("PROBE_DKG_SETUP_ROUTES"));
        assert!(smoke_script.contains("DKG_ROUTE_PROBE_ONLY"));
        assert!(smoke_script.contains("DKG_SETUP_PUBLIC_CONFIG"));
        assert!(smoke_script.contains("PUBLIC_CONFIG=\"$DKG_SETUP_PUBLIC_CONFIG\""));
        assert!(smoke_script.contains("PROBE_DKG_SETUP_ROUTES=1"));
        assert!(smoke_script.contains("dkg setup route probe passed"));
        assert!(smoke_script.contains("CDK_FEDERATION_DISABLE_PEER_SYNC=1"));
        assert!(smoke_script.contains("member_dkg_setup_request_dir()"));
        assert!(smoke_script.contains("member_mint_url()"));
        assert!(smoke_script.contains("member_federation_url()"));
        assert!(smoke_script.contains("probe_dkg_setup_routes()"));
        assert!(smoke_script.contains("post_dkg_setup_request()"));
        assert!(smoke_script.contains("wait_for_dkg_setup_records()"));
        assert!(smoke_script.contains("wait_for_runtime_keysets()"));
        assert!(smoke_script.contains("$mint_url/v1/keysets"));
        assert!(smoke_script.contains("did not expose activated DKG keysets after restart"));
        assert!(smoke_script.contains("member_dkg_completion_records_line()"));
        assert!(smoke_script.contains("could not post DKG setup route-probe request"));
        assert!(smoke_script.contains("/federation/v1/dkg/commitments"));
        assert!(smoke_script.contains("/federation/v1/dkg/reveals"));
        assert!(smoke_script.contains("/federation/v1/dkg/secret-shares"));
        assert!(smoke_script.contains("/federation/v1/dkg/transcript-signatures"));
        assert!(smoke_script.contains("keyset_share_records"));
        assert!(smoke_script.contains("transcript_signature_records"));
        assert!(smoke_script.contains(
            "probe_dkg_setup_routes\nwait_for_dkg_setup_records\nif [[ \"$DKG_ROUTE_PROBE_ONLY\" == \"1\" ]]; then"
        ));
        assert!(smoke_script.contains("wait_for_dkg_status \"$member\""));
        assert!(smoke_script.contains(
            "stop_member \"$member\"\n  done\n  for member in \"${MEMBERS[@]}\"; do\n    start_member \"$member\""
        ));
        assert!(smoke_script.contains("wait_for_runtime_keysets \"$member\""));
        assert!(smoke_script.contains("exit 0\nfi\n\nfor member in"));
        assert!(smoke_script.contains("wait_for_dkg_status()"));
        assert!(smoke_script.contains("print_dkg_record_runbook()"));
        assert!(smoke_script.contains("DKG record recovery for member"));
        assert!(smoke_script.contains("--keyset-share <keyset-share.json>"));
        assert!(smoke_script.contains("--transcript-signature <transcript-signature.json>"));
        assert!(smoke_script.contains("setup_records with collected commitment"));
        assert!(smoke_script.contains("Combine --from-database"));
        assert!(smoke_script.contains("member_quorum_status_line()"));
        assert!(smoke_script.contains("--require-session-outcome-quorum"));
        assert!(smoke_script.contains("--require-no-digest-alerts"));
        assert!(smoke_script.contains("private_config=\"$(member_private_config \"$member\")\""));
        assert!(!smoke_script
            .contains("private_config=\"$(member_private_config \"$PRIMARY_MEMBER\")\""));
        assert!(smoke_script.contains("reset_member_work_dirs"));
        assert!(smoke_script.contains("rm -rf \"$work_dir\""));
        assert!(smoke_script.contains("PRIMARY_MEMBER"));
        assert!(smoke_script.contains("RESTART_MEMBER"));
        assert!(smoke_script.contains("QUARANTINE_OFFLINE_MEMBER"));
        assert!(smoke_script.contains("unknown RESTART_MEMBER"));
        assert!(smoke_script.contains(
            "RESTART_MEMBER must differ from PRIMARY_MEMBER for offline operation smoke"
        ));
        assert!(smoke_script.contains("PENDING_WALLET_DB"));
        assert!(smoke_script.contains("FAILED_PENDING_WALLET_DB"));
        assert!(smoke_script.contains("SMOKE_QUOTE_RETRIES"));
        assert!(smoke_script.contains("SMOKE_QUOTE_SLEEP_MS"));
        assert!(smoke_script.contains("OFFLINE_OPERATION_BATCHES"));
        assert!(smoke_script.contains("CHECK_PROMETHEUS_METRICS"));
        assert!(smoke_script.contains("validate_smoke_settings()"));
        assert!(smoke_script.contains("OFFLINE_OPERATION_BATCHES must be a positive integer"));
        assert!(smoke_script.contains("OFFLINE_OPERATION_BATCHES must be 255 or lower"));
        assert!(smoke_script.contains("member_payment_report_lines()"));
        assert!(smoke_script.contains("--context mint-quote"));
        assert!(smoke_script.contains("member_prometheus_url()"));
        assert!(smoke_script.contains("run_smoke_operations()"));
        assert!(smoke_script.contains("local has_private_config=0"));
        assert!(smoke_script.contains("if [[ \"$arg\" == \"--private-config\" ]]; then"));
        assert!(smoke_script.contains("--private-config \"$(member_private_config \"$signer\")\""));
        assert!(smoke_script.contains("return\n  fi"));
        assert!(smoke_script.contains("wait_for_prometheus_metrics()"));
        assert!(smoke_script.contains("prometheus_metrics_are_ready()"));
        assert!(smoke_script.contains("cdk_federation_consensus_operation_count"));
        assert!(smoke_script.contains("cdk_federation_sync_ready"));
        assert!(smoke_script.contains("cdk_federation_sync_state"));
        assert!(smoke_script.contains("cdk_federation_payment_observation_reports"));
        assert!(smoke_script.contains("federation Prometheus metrics include a sensitive label"));
        assert!(smoke_script.contains("wait_for_session_outcome_quorum()"));
        assert!(smoke_script
            .contains("member %s did not report quorum-backed session outcome evidence"));
        assert!(smoke_script.contains("--quote-retries \"$SMOKE_QUOTE_RETRIES\""));
        assert!(smoke_script.contains("--quote-sleep-ms \"$SMOKE_QUOTE_SLEEP_MS\""));
        assert!(smoke_script.contains("wait_for_quarantine_report()"));
        assert!(smoke_script.contains("validate_quarantine_members()"));
        assert!(smoke_script.contains("--scenario offline-member-operations"));
        assert!(smoke_script.contains(
            "run_smoke_operations --scenario offline-member-operations --seed-offset \"$batch\""
        ));
        assert!(smoke_script.contains("--scenario pending-melt-start"));
        assert!(smoke_script.contains("--scenario pending-melt-finish"));
        assert!(smoke_script.contains("--scenario pending-melt-fail-start"));
        assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
        assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
        assert!(smoke_script.contains("--scenario offline-member-keyset-rotation"));
        assert!(smoke_script.contains("--wallet-db \"$PENDING_WALLET_DB\""));
        assert!(smoke_script.contains("--wallet-db \"$FAILED_PENDING_WALLET_DB\""));
        assert!(smoke_script.contains(
            "--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-finish"
        ));
        assert!(smoke_script.contains(
            "--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-fail-finish"
        ));
        assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-finish"
        ));
        assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-fail-finish"
        ));
        assert!(smoke_script
            .contains("--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\""));
        assert!(smoke_script.contains("wait_for_quarantine_report \"$PRIMARY_MEMBER\""));
        assert!(smoke_script.contains(
            "federation payment-observations --public-config \"$PUBLIC_CONFIG\" --private-config \"$private_config\" --member \"$member\" --context mint-quote"
        ));
        assert!(smoke_script.contains("stop_member \"$QUARANTINE_OFFLINE_MEMBER\""));
        assert!(smoke_script.contains("start_member \"$QUARANTINE_OFFLINE_MEMBER\""));
        assert!(smoke_script.contains("wait_for_status \"$QUARANTINE_OFFLINE_MEMBER\""));
        assert!(smoke_script.contains("wait_for_session_outcome_quorum \"$RESTART_MEMBER\""));
        assert!(
            smoke_script.contains("wait_for_session_outcome_quorum \"$QUARANTINE_OFFLINE_MEMBER\"")
        );
        assert!(smoke_script.contains(
            "run_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario offline-member-keyset-rotation"
        ));
        assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario offline-member-keyset-rotation"
        ));
        assert!(smoke_script.contains(
            "wait_for_status \"$RESTART_MEMBER\"\nwait_for_converged\nwait_for_session_outcome_quorum \"$RESTART_MEMBER\""
        ));
        assert!(smoke_script.contains(
            "wait_for_status \"$QUARANTINE_OFFLINE_MEMBER\"\n\nwait_for_converged\nwait_for_session_outcome_quorum \"$QUARANTINE_OFFLINE_MEMBER\""
        ));
        assert!(smoke_script.contains("wait_for_quarantine_report \"$QUARANTINE_OFFLINE_MEMBER\""));
        assert!(smoke_script.contains("local federation smoke test passed"));
        assert!(smoke_script.contains(&artifacts.public_config_path.to_string_lossy().to_string()));
        assert!(smoke_script.contains(
            &artifacts
                .dkg_setup_public_config_path
                .to_string_lossy()
                .to_string()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&artifacts.smoke_script_path)
                .expect("smoke script metadata")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0);
        }

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            let private_json = std::fs::read_to_string(&member_artifacts.private_config_path)
                .expect("private config file");
            let private_config: MemberConfigPrivate =
                serde_json::from_str(&private_json).expect("private config json");
            assert_eq!(private_config.member_id, *member_id);
            assert_eq!(
                private_config.federation_id,
                Some(public_config.federation_id)
            );
            assert_eq!(
                private_config.setup_transcript_hash,
                Some(setup_transcript.transcript_hash)
            );
            private_config
                .validate_against_public(&public_config)
                .expect("private config validates");
            let dkg_setup_private_json =
                std::fs::read_to_string(&member_artifacts.dkg_setup_private_config_path)
                    .expect("setup-only private config file");
            let dkg_setup_private_config: MemberConfigPrivate =
                serde_json::from_str(&dkg_setup_private_json)
                    .expect("setup-only private config json");
            assert_eq!(dkg_setup_private_config.member_id, *member_id);
            assert_eq!(
                dkg_setup_private_config.federation_id,
                Some(public_config.federation_id)
            );
            assert!(dkg_setup_private_config.keyset_secret_shares.is_empty());
            assert!(dkg_setup_private_config.setup_transcript_hash.is_none());
            dkg_setup_private_config
                .validate_against_public(&dkg_setup_public_config)
                .expect("setup-only private config validates");

            let config_text =
                std::fs::read_to_string(&member_artifacts.config_path).expect("member config text");
            let dkg_setup_config_text =
                std::fs::read_to_string(&member_artifacts.dkg_setup_config_path)
                    .expect("setup-only member config text");
            let expected_prometheus_port = 8085 + 3 + member_id.get() - 1;
            assert!(config_text.contains("[prometheus]"));
            assert!(config_text.contains("enabled = true"));
            assert!(config_text.contains("address = \"127.0.0.1\""));
            assert!(config_text.contains(&format!("port = {expected_prometheus_port}")));
            assert!(config_text.contains(&member_artifacts.federation_api_url));
            assert!(config_text.contains(&member_artifacts.public_mint_url));
            assert!(dkg_setup_config_text.contains(
                &artifacts
                    .dkg_setup_public_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(dkg_setup_config_text.contains(
                &member_artifacts
                    .dkg_setup_private_config_path
                    .to_string_lossy()
                    .to_string()
            ));

            let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
                .expect("member config toml");
            let dkg_setup_settings =
                config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                    .expect("setup-only member config toml");
            assert_eq!(settings.database.engine, config::DatabaseEngine::Sqlite);
            assert_eq!(
                dkg_setup_settings.database.engine,
                config::DatabaseEngine::Sqlite
            );
            assert!(
                settings
                    .federation
                    .as_ref()
                    .expect("federation settings")
                    .enabled
            );
            assert!(
                dkg_setup_settings
                    .federation
                    .as_ref()
                    .expect("setup-only federation settings")
                    .enabled
            );
            assert_eq!(settings.info.listen_port, 8084 + member_id.get());
            assert_eq!(dkg_setup_settings.info.listen_port, 8084 + member_id.get());
            #[cfg(feature = "prometheus")]
            {
                let prometheus = settings.prometheus.expect("prometheus settings");
                assert!(prometheus.enabled);
                assert_eq!(prometheus.address.as_deref(), Some("127.0.0.1"));
                assert_eq!(prometheus.port, Some(expected_prometheus_port));
            }
            assert!(
                smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string())
            );
            assert!(smoke_script.contains(
                &member_artifacts
                    .dkg_setup_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(
                &member_artifacts
                    .private_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(
                &member_artifacts
                    .dkg_setup_private_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
            assert!(smoke_script.contains(
                &member_artifacts
                    .dkg_setup_work_dir
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(&member_artifacts.public_mint_url));
            assert!(smoke_script.contains(&member_artifacts.prometheus_url));
        }

        let _ = std::fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn trusted_dealer_artifacts_support_postgres_database_template() {
        let output_dir = temp_output_dir("postgres_database_template");
        let _ = std::fs::remove_dir_all(&output_dir);
        let mut args = generator_args(output_dir.clone());
        args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
        args.postgres_url_template =
            Some("postgresql://cdk:cdk@localhost:5432/cdk_member_{member}".to_string());

        let artifacts =
            generate_trusted_dealer_artifacts(&args).expect("postgres trusted dealer artifacts");

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
                .expect("member config toml");
            let dkg_setup_settings =
                config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                    .expect("setup-only member config toml");
            assert_eq!(settings.database.engine, config::DatabaseEngine::Postgres);
            assert_eq!(
                dkg_setup_settings.database.engine,
                config::DatabaseEngine::Postgres
            );
            let postgres = settings
                .database
                .postgres
                .as_ref()
                .expect("postgres config");
            let dkg_setup_postgres = dkg_setup_settings
                .database
                .postgres
                .as_ref()
                .expect("setup-only postgres config");
            assert_eq!(
                postgres.url,
                format!(
                    "postgresql://cdk:cdk@localhost:5432/cdk_member_{}",
                    member_id.get()
                )
            );
            assert_eq!(dkg_setup_postgres.url, postgres.url);
            assert!(!postgres.url.contains("{member}"));
            assert!(!dkg_setup_postgres.url.contains("{member}"));
        }
        let smoke_script =
            std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
        assert!(smoke_script.contains("POSTGRES_SCHEMA_RESET_URL=''"));
        assert!(smoke_script.contains("reset_postgres_schemas()"));

        let _ = std::fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn trusted_dealer_artifacts_support_postgres_schema_base_url() {
        let output_dir = temp_output_dir("postgres_schema_base_url");
        let _ = std::fs::remove_dir_all(&output_dir);
        let mut args = generator_args(output_dir.clone());
        args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
        args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());

        let artifacts =
            generate_trusted_dealer_artifacts(&args).expect("postgres trusted dealer artifacts");

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
                .expect("member config toml");
            let dkg_setup_settings =
                config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                    .expect("setup-only member config toml");
            assert_eq!(settings.database.engine, config::DatabaseEngine::Postgres);
            assert_eq!(
                dkg_setup_settings.database.engine,
                config::DatabaseEngine::Postgres
            );
            let postgres = settings
                .database
                .postgres
                .as_ref()
                .expect("postgres config");
            let dkg_setup_postgres = dkg_setup_settings
                .database
                .postgres
                .as_ref()
                .expect("setup-only postgres config");
            assert_eq!(
                postgres.url,
                format!(
                    "postgresql://cdk:cdk@localhost:5432/cdk_mint schema=cdk_federation_member_{}",
                    member_id.get()
                )
            );
            assert_eq!(dkg_setup_postgres.url, postgres.url);
        }
        let smoke_script =
            std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
        assert!(smoke_script
            .contains("POSTGRES_SCHEMA_RESET_URL='postgresql://cdk:cdk@localhost:5432/cdk_mint'"));
        assert!(smoke_script.contains("RESET_POSTGRES_SCHEMAS=\"${RESET_POSTGRES_SCHEMAS:-0}\""));
        assert!(smoke_script.contains("generated PostgreSQL member schemas are configured"));
        assert!(smoke_script.contains("psql is required to reset PostgreSQL member schemas"));
        assert!(smoke_script.contains("DROP SCHEMA IF EXISTS"));
        assert!(smoke_script.contains("CREATE SCHEMA"));
        for member_id in artifacts.member_artifacts.keys() {
            assert!(smoke_script.contains(&format!("cdk_federation_member_{}", member_id.get())));
        }

        let _ = std::fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn trusted_dealer_postgres_storage_options_are_validated() {
        let output_dir = temp_output_dir("postgres_validation");
        let mut args = generator_args(output_dir);
        args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;

        let err = validate_trusted_dealer_args(&args).expect_err("postgres storage is required");
        assert!(err
            .to_string()
            .contains("--postgres-url-template or --postgres-base-url is required"));

        args.postgres_url_template = Some("postgresql://cdk:cdk@localhost:5432/shared".to_string());
        let err = validate_trusted_dealer_args(&args).expect_err("member placeholder is required");
        assert!(err.to_string().contains("must contain {member}"));

        args.postgres_url_template =
            Some("postgresql://cdk:cdk@localhost:5432/cdk_member_{member}".to_string());
        args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());
        let err = validate_trusted_dealer_args(&args).expect_err("only one postgres option");
        assert!(err.to_string().contains("not both"));

        args.postgres_url_template = None;
        args.postgres_base_url = Some("   ".to_string());
        let err = validate_trusted_dealer_args(&args).expect_err("base url cannot be empty");
        assert!(err
            .to_string()
            .contains("--postgres-base-url must not be empty"));

        args.database_engine = FederationTrustedDealerDatabaseEngine::Sqlite;
        args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());
        let err = validate_trusted_dealer_args(&args).expect_err("sqlite rejects postgres options");
        assert!(err
            .to_string()
            .contains("require --database-engine postgres"));
    }

    #[test]
    fn trusted_dealer_smoke_script_supports_five_member_matrix() {
        let output_dir = temp_output_dir("five_member_matrix");
        let _ = std::fs::remove_dir_all(&output_dir);
        let mut args = generator_args(output_dir.clone());
        args.members = 5;
        args.threshold = 3;
        args.port_start = 19_565;

        let artifacts =
            generate_trusted_dealer_artifacts(&args).expect("five member trusted dealer artifacts");
        let public_json =
            std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
        let public_config: FederationConfig =
            serde_json::from_str(&public_json).expect("public config json");
        public_config.validate().expect("public config validates");
        assert_eq!(public_config.threshold_params.member_count, 5);
        assert_eq!(public_config.threshold_params.signature_threshold, 3);
        assert_eq!(artifacts.member_artifacts.len(), 5);

        let smoke_script =
            std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
        assert!(smoke_script.contains("MEMBERS=(1 2 3 4 5)"));
        assert!(smoke_script.contains("PRIMARY_MEMBER=\"${PRIMARY_MEMBER:-1}\""));
        assert!(smoke_script.contains("RESTART_MEMBER=\"${RESTART_MEMBER:-2}\""));
        assert!(smoke_script.contains(
            "QUARANTINE_OFFLINE_MEMBER=\"${QUARANTINE_OFFLINE_MEMBER:-$RESTART_MEMBER}\""
        ));
        assert!(
            smoke_script.contains("OFFLINE_OPERATION_BATCHES=\"${OFFLINE_OPERATION_BATCHES:-3}\"")
        );
        assert!(
            smoke_script.contains("CHECK_PROMETHEUS_METRICS=\"${CHECK_PROMETHEUS_METRICS:-1}\"")
        );
        assert!(smoke_script.contains("for member in \"${MEMBERS[@]}\"; do"));
        assert!(smoke_script.contains("for batch in $(seq 1 \"$OFFLINE_OPERATION_BATCHES\"); do"));
        assert!(smoke_script.contains("wait_for_converged"));
        assert!(smoke_script.contains("wait_for_prometheus_metrics"));
        assert!(smoke_script.contains("wait_for_session_outcome_quorum"));
        assert!(smoke_script.contains("--scenario offline-member-operations"));
        assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
        assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
        assert!(smoke_script.contains("federation payment-observations"));

        for (member_id, member_artifacts) in &artifacts.member_artifacts {
            assert!(smoke_script.contains(&format!("    {}) printf", member_id.get())));
            assert!(
                smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string())
            );
            assert!(smoke_script.contains(
                &member_artifacts
                    .private_config_path
                    .to_string_lossy()
                    .to_string()
            ));
            assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
            assert!(smoke_script.contains(&member_artifacts.prometheus_url));
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
