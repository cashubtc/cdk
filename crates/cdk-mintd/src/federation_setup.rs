#[cfg(feature = "federation")]
use std::collections::BTreeMap;
#[cfg(feature = "federation")]
use std::io::{self, Write};
#[cfg(feature = "federation")]
use std::path::{Path, PathBuf};
#[cfg(feature = "federation")]
use std::str::FromStr;
#[cfg(feature = "federation")]
use std::time::Instant;

#[cfg(feature = "federation")]
use anyhow::{anyhow, bail, Context, Result};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use bip39::Mnemonic;
#[cfg(feature = "federation")]
use bitcoin::hashes::{sha256, Hash};
#[cfg(feature = "federation")]
use cdk_common::federation::{
    generate_trusted_dealer_configs_with_options, FederatedKeyset, FederationConfig,
    FederationDkgCeremonyId, FederationDkgCommitmentRequest, FederationDkgCommitmentRound,
    FederationDkgKeysetSpec, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgPublicCeremony,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupMethod,
    FederationDkgTranscript, FederationDkgTranscriptSignature,
    FederationDkgTranscriptSignatureRequest, FederationRestMessageKind, FederationSignedRequest,
    MemberConfigPrivate, MemberId, ThresholdParams, TrustedDealerConfigBundle,
    TrustedDealerGenerationOptions, TrustedDealerKeysetSpec, TrustedDealerMember,
};
#[cfg(feature = "federation")]
use cdk_common::mint_url::MintUrl;
#[cfg(feature = "federation")]
use cdk_common::nuts::nut01::{
    aggregate_secret_key_shares, evaluate_public_key_commitments, evaluate_secret_key_coefficients,
    BlsSecretKey, BlsSecretKeyShare,
};
#[cfg(feature = "federation")]
use cdk_common::{Amount, CurrencyUnit, SecretKey};

#[cfg(feature = "federation")]
use crate::cli::{FederationTrustedDealerArgs, FederationTrustedDealerDatabaseEngine};

#[cfg(feature = "federation")]
const PUBLIC_CONFIG_FILE: &str = "federation-public.json";
#[cfg(feature = "federation")]
const DKG_SETUP_PUBLIC_CONFIG_FILE: &str = "dkg-setup-public.json";
#[cfg(feature = "federation")]
const PRIVATE_CONFIG_FILE: &str = "federation-private.json";
#[cfg(feature = "federation")]
const DKG_SETUP_PRIVATE_CONFIG_FILE: &str = "dkg-setup-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_FILE: &str = "config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_MEMBER_CONFIG_FILE: &str = "dkg-setup-config.toml";
#[cfg(feature = "federation")]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_FILE: &str = "smoke-local-federation.sh";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_MEMBER_CONFIG_FILE: &str = "dkg-live-config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_FINAL_CONFIG_DIR: &str = "dkg-final";
#[cfg(feature = "federation")]
const DKG_SETUP_ROUTE_PROBE_DIR: &str = "dkg-setup-route-probe";

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerCoreArtifacts {
    pub(crate) public_config_path: PathBuf,
    pub(crate) dkg_setup_public_config_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerCoreMemberArtifacts>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerCoreMemberArtifacts {
    pub(crate) private_config_path: PathBuf,
    pub(crate) dkg_setup_private_config_path: PathBuf,
    pub(crate) dkg_setup_request_dir: PathBuf,
}

#[cfg(feature = "federation")]
#[derive(Debug)]
#[allow(dead_code)]
struct TrustedDealerCoreGeneration {
    core_artifacts: TrustedDealerCoreArtifacts,
    output_dir: PathBuf,
    unit: CurrencyUnit,
    bundle: TrustedDealerConfigBundle,
}

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
    pub(crate) live_dkg_config_path: PathBuf,
    pub(crate) private_config_path: PathBuf,
    pub(crate) dkg_setup_private_config_path: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub(crate) dkg_setup_work_dir: PathBuf,
    pub(crate) live_dkg_work_dir: PathBuf,
    pub(crate) live_dkg_final_config_dir: PathBuf,
    pub(crate) public_mint_url: String,
    pub(crate) federation_api_url: String,
    pub(crate) dkg_setup_request_dir: PathBuf,
    pub(crate) prometheus_url: String,
}

#[cfg(feature = "federation")]
fn print_trusted_dealer_stage(stage: &str, started_at: Instant) {
    println!(
        "federation trusted-dealer stage: stage={} elapsed_ms={}",
        stage,
        started_at.elapsed().as_millis()
    );
    let _ = io::stdout().flush();
}

#[cfg(feature = "federation")]
pub(crate) fn run_trusted_dealer_command(args: &FederationTrustedDealerArgs) -> Result<()> {
    #[cfg(feature = "fakewallet")]
    if args.local_fakewallet_configs {
        let artifacts = generate_trusted_dealer_artifacts(args)?;
        println!(
            "Wrote federation public config: {}",
            artifacts.public_config_path.display()
        );
        for (member_id, member) in &artifacts.member_artifacts {
            println!(
                "Wrote member {} local fakewallet config: {}",
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

        return Ok(());
    }

    let artifacts = generate_trusted_dealer_core_artifacts(args)?;
    println!(
        "Wrote federation public config: {}",
        artifacts.public_config_path.display()
    );
    println!(
        "Wrote DKG setup public config: {}",
        artifacts.dkg_setup_public_config_path.display()
    );
    for (member_id, member) in &artifacts.member_artifacts {
        println!(
            "Wrote member {} private config: {}",
            member_id,
            member.private_config_path.display()
        );
        println!(
            "Wrote member {} DKG setup private config: {}",
            member_id,
            member.dkg_setup_private_config_path.display()
        );
    }
    #[cfg(feature = "fakewallet")]
    println!(
        "Skipped local fakewallet member TOMLs and smoke script; pass --local-fakewallet-configs for development/regtest files."
    );

    Ok(())
}

#[cfg(feature = "federation")]
#[cfg_attr(feature = "fakewallet", allow(dead_code))]
pub(crate) fn generate_trusted_dealer_core_artifacts(
    args: &FederationTrustedDealerArgs,
) -> Result<TrustedDealerCoreArtifacts> {
    Ok(generate_trusted_dealer_core_generation(args)?.core_artifacts)
}

#[cfg(feature = "federation")]
fn generate_trusted_dealer_core_generation(
    args: &FederationTrustedDealerArgs,
) -> Result<TrustedDealerCoreGeneration> {
    let setup_started_at = Instant::now();
    print_trusted_dealer_stage("setup.start", setup_started_at);
    validate_trusted_dealer_args(args)?;

    print_trusted_dealer_stage("setup.output-dir.start", setup_started_at);
    let output_dir = create_output_dir(&args.output_dir)?;
    write_generated_gitignore(&output_dir, args.force)?;
    print_trusted_dealer_stage("setup.output-dir.done", setup_started_at);

    print_trusted_dealer_stage("setup.roster.start", setup_started_at);
    let threshold_params = ThresholdParams::with_bft_consensus(args.members, args.threshold)?;
    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation unit {}", args.unit))?;
    let deterministic_test_seed = args.deterministic_test_seed.as_deref();
    let members = trusted_dealer_members(
        args.members,
        &args.host,
        args.port_start,
        deterministic_test_seed,
    )?;
    print_trusted_dealer_stage("setup.roster.done", setup_started_at);

    print_trusted_dealer_stage("setup.active-keyset.start", setup_started_at);
    let keyset_spec = trusted_dealer_keyset_spec(
        unit.clone(),
        true,
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
        deterministic_test_seed,
        "active",
    )?;
    print_trusted_dealer_stage("setup.active-keyset.done", setup_started_at);

    print_trusted_dealer_stage("setup.rotation-keyset.start", setup_started_at);
    let rotation_keyset_spec = trusted_dealer_keyset_spec(
        unit.clone(),
        false,
        args.input_fee_ppk,
        args.final_expiry,
        args.threshold,
        &args.amounts,
        deterministic_test_seed,
        "rotation",
    )?;
    print_trusted_dealer_stage("setup.rotation-keyset.done", setup_started_at);

    print_trusted_dealer_stage("setup.bundle.start", setup_started_at);
    let bundle = generate_trusted_dealer_configs_with_options(
        threshold_params,
        members,
        vec![keyset_spec, rotation_keyset_spec],
        TrustedDealerGenerationOptions {
            no_aux_rand_signatures: deterministic_test_seed.is_some(),
        },
    )
    .context("failed to generate trusted-dealer federation configs")?;
    print_trusted_dealer_stage("setup.bundle.done", setup_started_at);

    print_trusted_dealer_stage("setup.validate.start", setup_started_at);
    validate_bundle(&bundle)?;
    let dkg_setup_public_config = dkg_setup_public_config(&bundle.public_config)?;
    print_trusted_dealer_stage("setup.validate.done", setup_started_at);

    let dkg_setup_request_dirs = if args.dkg_setup_route_probes {
        print_trusted_dealer_stage("setup.dkg-probes.start", setup_started_at);
        let request_dirs = write_dkg_setup_route_probe_requests(
            &output_dir,
            &bundle.public_config,
            &bundle.private_configs,
            &unit,
            args.input_fee_ppk,
            args.final_expiry,
            args.threshold,
            &args.amounts,
            deterministic_test_seed,
            args.force,
        )?;
        print_trusted_dealer_stage("setup.dkg-probes.done", setup_started_at);
        request_dirs
    } else {
        print_trusted_dealer_stage("setup.dkg-probes.skipped", setup_started_at);
        dkg_setup_route_probe_dirs(&output_dir, &bundle.public_config)
    };

    print_trusted_dealer_stage("setup.public-files.start", setup_started_at);
    let public_config_path = output_dir.join(PUBLIC_CONFIG_FILE);
    write_json_file(&public_config_path, &bundle.public_config, args.force)?;
    let dkg_setup_public_config_path = output_dir.join(DKG_SETUP_PUBLIC_CONFIG_FILE);
    write_json_file(
        &dkg_setup_public_config_path,
        &dkg_setup_public_config,
        args.force,
    )?;
    print_trusted_dealer_stage("setup.public-files.done", setup_started_at);

    print_trusted_dealer_stage("setup.member-files.start", setup_started_at);
    let mut core_member_artifacts = BTreeMap::new();
    for (member_id, private_config) in &bundle.private_configs {
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

        core_member_artifacts.insert(
            *member_id,
            TrustedDealerCoreMemberArtifacts {
                private_config_path,
                dkg_setup_private_config_path,
                dkg_setup_request_dir: dkg_setup_request_dirs
                    .get(member_id)
                    .ok_or_else(|| anyhow!("missing DKG route-probe dir for member {member_id}"))?
                    .clone(),
            },
        );
    }
    print_trusted_dealer_stage("setup.member-files.done", setup_started_at);

    Ok(TrustedDealerCoreGeneration {
        core_artifacts: TrustedDealerCoreArtifacts {
            public_config_path,
            dkg_setup_public_config_path,
            member_artifacts: core_member_artifacts,
        },
        output_dir,
        unit,
        bundle,
    })
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
pub(crate) fn generate_trusted_dealer_artifacts(
    args: &FederationTrustedDealerArgs,
) -> Result<TrustedDealerArtifacts> {
    let generation = generate_trusted_dealer_core_generation(args)?;
    let setup_started_at = Instant::now();
    print_trusted_dealer_stage("setup.local-member-files.start", setup_started_at);

    let public_config_path = generation.core_artifacts.public_config_path.clone();
    let dkg_setup_public_config_path = generation
        .core_artifacts
        .dkg_setup_public_config_path
        .clone();
    let live_dkg_keyset_spec = FederationDkgKeysetSpec::new(
        generation.unit.clone(),
        true,
        args.input_fee_ppk,
        args.final_expiry,
        args.amounts.iter().copied().map(Amount::from).collect(),
    )
    .context("invalid generated live DKG keyset policy")?;
    let live_dkg_ceremony_id = generated_dkg_ceremony_id(
        b"cdk-federation-live-dkg-ceremony-v1",
        &generation.bundle.public_config,
        &live_dkg_keyset_spec,
        args.deterministic_test_seed.as_deref(),
    )?;
    let live_dkg_config_suffix =
        live_dkg_config_toml(live_dkg_ceremony_id, &live_dkg_keyset_spec, &args.host)?;
    let mut member_artifacts = BTreeMap::new();
    for member_id in generation.bundle.private_configs.keys() {
        let member = generation
            .bundle
            .public_config
            .member(*member_id)
            .ok_or_else(|| anyhow!("generated bundle is missing member {}", member_id))?;
        let core_member = generation
            .core_artifacts
            .member_artifacts
            .get(member_id)
            .ok_or_else(|| anyhow!("missing core artifacts for member {member_id}"))?;
        let member_dir = generation
            .output_dir
            .join(format!("member-{}", member_id.get()));
        let work_dir = member_dir.join("work");
        let dkg_setup_work_dir = member_dir.join("dkg-setup-work");
        let live_dkg_work_dir = member_dir.join("dkg-live-work");
        let live_dkg_final_config_dir = member_dir.join(LIVE_DKG_FINAL_CONFIG_DIR);
        let config_path = member_dir.join(MEMBER_CONFIG_FILE);
        let dkg_setup_config_path = member_dir.join(DKG_SETUP_MEMBER_CONFIG_FILE);
        let live_dkg_config_path = member_dir.join(LIVE_DKG_MEMBER_CONFIG_FILE);
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
            &core_member.private_config_path,
            &config_path,
            &work_dir,
            &generation.unit,
            &database_config,
            args.deterministic_test_seed.as_deref(),
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
            &core_member.dkg_setup_private_config_path,
            &dkg_setup_config_path,
            &dkg_setup_work_dir,
            &generation.unit,
            &database_config,
            args.deterministic_test_seed.as_deref(),
        )?;
        write_text_file(&dkg_setup_config_path, &dkg_setup_config_toml, args.force)?;
        let mut live_dkg_config_toml = member_config_toml(
            member_id,
            &args.host,
            &member_url,
            &federation_api_url,
            listen_port,
            prometheus_port,
            &dkg_setup_public_config_path,
            &core_member.dkg_setup_private_config_path,
            &live_dkg_config_path,
            &live_dkg_work_dir,
            &generation.unit,
            &database_config,
            args.deterministic_test_seed.as_deref(),
        )?;
        live_dkg_config_toml.push_str(&live_dkg_config_suffix);
        write_text_file(&live_dkg_config_path, &live_dkg_config_toml, args.force)?;

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                dkg_setup_config_path,
                live_dkg_config_path,
                private_config_path: core_member.private_config_path.clone(),
                dkg_setup_private_config_path: core_member.dkg_setup_private_config_path.clone(),
                work_dir,
                dkg_setup_work_dir,
                live_dkg_work_dir,
                live_dkg_final_config_dir,
                public_mint_url: member_url,
                federation_api_url,
                dkg_setup_request_dir: core_member.dkg_setup_request_dir.clone(),
                prometheus_url,
            },
        );
    }
    print_trusted_dealer_stage("setup.local-member-files.done", setup_started_at);

    print_trusted_dealer_stage("setup.smoke-script.start", setup_started_at);
    let smoke_script_path = generation.output_dir.join(SMOKE_SCRIPT_FILE);
    let smoke_script = local_smoke_script(
        &public_config_path,
        &dkg_setup_public_config_path,
        &member_artifacts,
        args,
    )?;
    write_executable_text_file(&smoke_script_path, &smoke_script, args.force)?;
    print_trusted_dealer_stage("setup.smoke-script.done", setup_started_at);
    print_trusted_dealer_stage("setup.done", setup_started_at);

    Ok(TrustedDealerArtifacts {
        public_config_path,
        dkg_setup_public_config_path,
        smoke_script_path,
        member_artifacts,
    })
}

#[cfg(feature = "federation")]
fn dkg_setup_public_config(public_config: &FederationConfig) -> Result<FederationConfig> {
    let mut setup_public_config = public_config.clone();
    setup_public_config.keysets.clear();
    setup_public_config.setup_transcript = None;
    setup_public_config
        .validate()
        .context("generated setup-only public federation config is invalid")?;
    Ok(setup_public_config)
}

#[cfg(feature = "federation")]
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

#[cfg(feature = "federation")]
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
    if matches!(
        args.deterministic_test_seed.as_deref(),
        Some(seed) if seed.trim().is_empty()
    ) {
        bail!("--deterministic-test-seed must not be empty");
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

#[cfg(feature = "federation")]
fn create_output_dir(output_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    output_dir
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", output_dir.display()))
}

#[cfg(feature = "federation")]
fn write_generated_gitignore(output_dir: &Path, force: bool) -> Result<()> {
    write_text_file(
        &output_dir.join(GENERATED_GITIGNORE_FILE),
        "# Generated local federation material may contain secrets.\n*\n",
        force,
    )
}

#[cfg(feature = "federation")]
fn trusted_dealer_members(
    member_count: u16,
    host: &str,
    port_start: u16,
    deterministic_test_seed: Option<&str>,
) -> Result<Vec<TrustedDealerMember>> {
    (1..=member_count)
        .map(|id| {
            let member_id = MemberId::new(id)?;
            let port = port_start
                .checked_add(id - 1)
                .ok_or_else(|| anyhow!("member ports overflow u16"))?;
            let url = MintUrl::from_str(&format!("http://{host}:{port}"))
                .with_context(|| format!("invalid generated URL for member {member_id}"))?;
            let identity_secret_key = match deterministic_test_seed {
                Some(seed) => deterministic_test_secp256k1_secret(
                    seed,
                    "member-identity",
                    &[&member_id.get().to_string()],
                )?,
                None => SecretKey::generate(),
            };
            Ok(TrustedDealerMember::new(
                member_id,
                url.clone(),
                url,
                identity_secret_key,
            ))
        })
        .collect()
}

#[cfg(feature = "federation")]
fn trusted_dealer_keyset_spec(
    unit: CurrencyUnit,
    active: bool,
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    threshold: u16,
    amounts: &[u64],
    deterministic_test_seed: Option<&str>,
    keyset_label: &str,
) -> Result<TrustedDealerKeysetSpec> {
    let mut amount_coefficients = BTreeMap::new();
    for amount in amounts {
        let coefficients = bls_coefficients(
            threshold,
            deterministic_test_seed,
            &[keyset_label, &amount.to_string()],
        )?;
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

#[cfg(feature = "federation")]
fn bls_coefficients(
    threshold: u16,
    deterministic_test_seed: Option<&str>,
    labels: &[&str],
) -> Result<Vec<BlsSecretKey>> {
    match deterministic_test_seed {
        Some(seed) => (0..threshold)
            .map(|coefficient_index| {
                let coefficient_index = coefficient_index.to_string();
                let mut coefficient_labels = labels.to_vec();
                coefficient_labels.push(&coefficient_index);
                Ok(deterministic_test_bls_secret(
                    seed,
                    "bls-coefficient",
                    &coefficient_labels,
                ))
            })
            .collect(),
        None => (0..threshold)
            .map(|_| {
                let secret_key = SecretKey::generate_bls();
                Ok(secret_key.as_bls()?.clone())
            })
            .collect(),
    }
}

#[cfg(feature = "federation")]
fn deterministic_test_bytes(seed: &str, domain: &str, labels: &[&str]) -> [u8; 32] {
    let mut transcript = Vec::new();
    append_deterministic_test_field(
        &mut transcript,
        b"cdk-federation-trusted-dealer-test-seed-v1",
    );
    append_deterministic_test_field(&mut transcript, seed.as_bytes());
    append_deterministic_test_field(&mut transcript, domain.as_bytes());
    for label in labels {
        append_deterministic_test_field(&mut transcript, label.as_bytes());
    }

    sha256::Hash::hash(&transcript).to_byte_array()
}

#[cfg(feature = "federation")]
fn append_deterministic_test_field(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u64).to_be_bytes());
    transcript.extend_from_slice(value);
}

#[cfg(feature = "federation")]
fn deterministic_test_secp256k1_secret(
    seed: &str,
    domain: &str,
    labels: &[&str],
) -> Result<SecretKey> {
    for attempt in 0..16 {
        let attempt = attempt.to_string();
        let mut attempt_labels = labels.to_vec();
        attempt_labels.push(&attempt);
        let bytes = deterministic_test_bytes(seed, domain, &attempt_labels);
        if let Ok(secret_key) = SecretKey::from_slice(&bytes) {
            return Ok(secret_key);
        }
    }

    bail!("failed to derive deterministic secp256k1 key for {domain} after 16 attempts");
}

#[cfg(feature = "federation")]
fn deterministic_test_bls_secret(seed: &str, domain: &str, labels: &[&str]) -> BlsSecretKey {
    let bytes = deterministic_test_bytes(seed, domain, labels);
    BlsSecretKey::from_reduced_bytes(&bytes)
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn deterministic_test_mnemonic(seed: &str, labels: &[&str]) -> Result<Mnemonic> {
    let entropy = deterministic_test_bytes(seed, "member-mnemonic", labels);
    Mnemonic::from_entropy(&entropy[..16])
        .map_err(|err| anyhow!("failed to derive deterministic member mnemonic: {err}"))
}

#[cfg(feature = "federation")]
fn dkg_setup_route_probe_dirs(
    output_dir: &Path,
    public_config: &FederationConfig,
) -> BTreeMap<MemberId, PathBuf> {
    public_config
        .members
        .keys()
        .map(|member_id| {
            (
                *member_id,
                output_dir
                    .join(DKG_SETUP_ROUTE_PROBE_DIR)
                    .join(format!("receiver-{}", member_id.get())),
            )
        })
        .collect()
}

#[cfg(feature = "federation")]
fn write_dkg_setup_route_probe_requests(
    output_dir: &Path,
    public_config: &FederationConfig,
    private_configs: &BTreeMap<MemberId, MemberConfigPrivate>,
    unit: &CurrencyUnit,
    input_fee_ppk: u64,
    final_expiry: Option<u64>,
    threshold: u16,
    amounts: &[u64],
    deterministic_test_seed: Option<&str>,
    force: bool,
) -> Result<BTreeMap<MemberId, PathBuf>> {
    let deterministic_test_signatures = deterministic_test_seed.is_some();
    let keyset_spec = FederationDkgKeysetSpec::new(
        unit.clone(),
        true,
        input_fee_ppk,
        final_expiry,
        amounts.iter().copied().map(Amount::from).collect(),
    )?;
    let coefficients =
        dkg_probe_coefficients(public_config, threshold, amounts, deterministic_test_seed)?;
    let ceremony_id = dkg_probe_ceremony_id(public_config, &keyset_spec, deterministic_test_seed)?;
    let reveals = coefficients
        .keys()
        .map(|member_id| {
            Ok((
                *member_id,
                dkg_probe_reveal(
                    public_config,
                    ceremony_id,
                    *member_id,
                    &keyset_spec,
                    &coefficients,
                )?,
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

    let mut secret_share_payloads = BTreeMap::new();
    for receiver_id in public_config.members.keys().copied() {
        let mut receiver_payloads = BTreeMap::new();
        for sender_id in public_config.members.keys().copied() {
            let secret_share_payload = dkg_probe_secret_share_request(
                public_config,
                &coefficients,
                ceremony_id,
                sender_id,
                receiver_id,
            )?;
            receiver_payloads.insert(sender_id, secret_share_payload);
        }
        secret_share_payloads.insert(receiver_id, receiver_payloads);
    }

    let route_probe_keyset = round
        .derive_federated_keyset(public_config, &reveals)
        .context("generated DKG route-probe final keyset is invalid")?;
    for (receiver_id, receiver_payloads) in &secret_share_payloads {
        let participant_secret_shares = receiver_payloads
            .iter()
            .map(|(sender_id, payload)| (*sender_id, payload.secret_shares.clone()))
            .collect::<BTreeMap<_, _>>();
        validate_dkg_probe_member_secret_shares(
            public_config,
            &round,
            &reveals,
            &route_probe_keyset,
            *receiver_id,
            &participant_secret_shares,
        )
        .with_context(|| {
            format!("generated DKG route-probe shares for member {receiver_id} are invalid")
        })?;
    }
    let final_public_config = FederationConfig::new(
        public_config.threshold_params,
        public_config.members.clone(),
        vec![route_probe_keyset],
    )
    .context("generated DKG route-probe final public config is invalid")?;
    let public_ceremonies =
        vec![
            FederationDkgPublicCeremony::new(public_config, round.clone(), reveals.clone())
                .context("generated DKG route-probe public ceremony is invalid")?,
        ];
    let transcript_hash = FederationDkgTranscript::hash_for_config(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
    )
    .context("generated DKG route-probe transcript hash is invalid")?;
    let transcript_signature_payloads = private_configs
        .iter()
        .map(|(sender_id, private_config)| {
            let signature = match deterministic_test_signatures {
                true => FederationDkgTranscriptSignature::sign_no_aux_rand(
                    transcript_hash,
                    private_config,
                )?,
                false => FederationDkgTranscriptSignature::sign(transcript_hash, private_config)?,
            };
            Ok((
                *sender_id,
                FederationDkgTranscriptSignatureRequest::new(
                    final_public_config.federation_id,
                    *sender_id,
                    transcript_hash,
                    signature,
                ),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let transcript = FederationDkgTranscript::from_signatures(
        FederationDkgSetupMethod::PedersenDkg,
        &final_public_config,
        &public_ceremonies,
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
                deterministic_test_signatures,
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
                deterministic_test_signatures,
                force,
            )?;

            let secret_share_payload = secret_share_payloads
                .get(&receiver_id)
                .and_then(|receiver_payloads| receiver_payloads.get(&sender_id))
                .ok_or_else(|| {
                    anyhow!(
                        "missing DKG route-probe secret-share payload from member {sender_id} to member {receiver_id}"
                    )
                })?
                .clone();
            write_signed_dkg_probe_request(
                public_config,
                private_config,
                receiver_id,
                3,
                FederationRestMessageKind::DkgSecretShare,
                secret_share_payload,
                &dkg_probe_request_path(&receiver_dir, sender_id, "secret-share"),
                deterministic_test_signatures,
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
                deterministic_test_signatures,
                force,
            )?;
        }
    }

    Ok(request_dirs)
}

#[cfg(feature = "federation")]
fn validate_dkg_probe_member_secret_shares(
    public_config: &FederationConfig,
    round: &FederationDkgCommitmentRound,
    reveals: &BTreeMap<MemberId, FederationDkgParticipantCommitmentReveal>,
    keyset: &FederatedKeyset,
    member_id: MemberId,
    participant_secret_shares: &BTreeMap<MemberId, BTreeMap<Amount, BlsSecretKeyShare>>,
) -> Result<()> {
    if !public_config.members.contains_key(&member_id) {
        bail!("generated DKG route-probe is missing member {member_id}");
    }
    let signer_id = member_id.to_bls_signer_id();
    let public_member_shares = keyset.public_shares.get(&member_id).ok_or_else(|| {
        anyhow!("generated DKG route-probe keyset is missing public shares for member {member_id}")
    })?;

    for amount in keyset.aggregate_keys.keys() {
        let mut participant_shares = Vec::with_capacity(round.participant_order.len());
        for participant_id in &round.participant_order {
            let participant_amounts = participant_secret_shares.get(participant_id).ok_or_else(|| {
                anyhow!(
                    "generated DKG route-probe is missing local secret contributions from member {participant_id}"
                )
            })?;
            let secret_share = participant_amounts.get(amount).ok_or_else(|| {
                anyhow!(
                    "generated DKG route-probe is missing local secret contribution from member {participant_id} for amount {amount}"
                )
            })?;
            if secret_share.signer_id() != signer_id {
                bail!(
                    "generated DKG route-probe secret contribution signer mismatch from member {participant_id} for receiver {member_id} amount {amount}"
                );
            }

            let commitments = reveals
                .get(participant_id)
                .and_then(|reveal| reveal.coefficient_commitments.get(amount))
                .ok_or_else(|| {
                    anyhow!(
                        "generated DKG route-probe is missing public commitments from member {participant_id} for amount {amount}"
                    )
                })?;
            let expected_public_share = evaluate_public_key_commitments(signer_id, commitments)?;
            if secret_share.public_key_share() != expected_public_share {
                bail!(
                    "generated DKG route-probe local secret contribution mismatch from member {participant_id} for receiver {member_id} amount {amount}"
                );
            }
            participant_shares.push(secret_share.clone());
        }

        let secret_share = aggregate_secret_key_shares(signer_id, &participant_shares)?;
        let public_share = public_member_shares.get(amount).ok_or_else(|| {
            anyhow!(
                "generated DKG route-probe keyset is missing public share for member {member_id} amount {amount}"
            )
        })?;
        if secret_share.public_key_share() != *public_share {
            bail!(
                "generated DKG route-probe aggregate secret share mismatch for member {member_id} amount {amount}"
            );
        }
    }

    Ok(())
}

#[cfg(feature = "federation")]
fn dkg_probe_coefficients(
    public_config: &FederationConfig,
    threshold: u16,
    amounts: &[u64],
    deterministic_test_seed: Option<&str>,
) -> Result<BTreeMap<MemberId, BTreeMap<Amount, Vec<BlsSecretKey>>>> {
    let mut coefficients = BTreeMap::new();
    for member_id in public_config.members.keys().copied() {
        let mut member_coefficients = BTreeMap::new();
        for amount in amounts {
            let member_label = member_id.get().to_string();
            if member_coefficients
                .insert(
                    Amount::from(*amount),
                    bls_coefficients(
                        threshold,
                        deterministic_test_seed,
                        &["dkg-route-probe", &member_label, &amount.to_string()],
                    )?,
                )
                .is_some()
            {
                bail!("duplicate federation amount {amount}");
            }
        }
        coefficients.insert(member_id, member_coefficients);
    }
    Ok(coefficients)
}

#[cfg(feature = "federation")]
fn dkg_probe_ceremony_id(
    public_config: &FederationConfig,
    keyset_spec: &FederationDkgKeysetSpec,
    deterministic_test_seed: Option<&str>,
) -> Result<FederationDkgCeremonyId> {
    generated_dkg_ceremony_id(
        b"cdk-federation-dkg-route-probe-ceremony-v1",
        public_config,
        keyset_spec,
        deterministic_test_seed,
    )
}

#[cfg(feature = "federation")]
fn generated_dkg_ceremony_id(
    domain: &[u8],
    public_config: &FederationConfig,
    keyset_spec: &FederationDkgKeysetSpec,
    deterministic_test_seed: Option<&str>,
) -> Result<FederationDkgCeremonyId> {
    let entropy = match deterministic_test_seed {
        Some(seed) => seed.as_bytes().to_vec(),
        None => SecretKey::generate().public_key().to_bytes(),
    };
    let mut material = Vec::new();
    material.extend_from_slice(domain);
    material.extend_from_slice(&public_config.federation_id.to_bytes());
    material.extend_from_slice(&serde_json::to_vec(keyset_spec)?);
    material.extend_from_slice(&entropy);
    Ok(FederationDkgCeremonyId::from_bytes(
        sha256::Hash::hash(&material).to_byte_array(),
    ))
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn live_dkg_config_toml(
    ceremony_id: FederationDkgCeremonyId,
    keyset_spec: &FederationDkgKeysetSpec,
    host: &str,
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
    let allow_insecure_localhost = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());

    Ok(format!(
        r#"
[federation.dkg]
enabled = true
ceremony_id = {ceremony_id}
unit = {unit}
active = {active}
input_fee_ppk = {input_fee_ppk}
{expiry}amounts = [{amounts}]
max_ticks = 1200
retry_delay_ms = 250
allow_insecure_localhost = {allow_insecure_localhost}
"#,
        ceremony_id = toml_string(&ceremony_id.to_string())?,
        unit = toml_string(&keyset_spec.unit.to_string().to_lowercase())?,
        active = keyset_spec.active,
        input_fee_ppk = keyset_spec.input_fee_ppk,
        expiry = expiry,
        amounts = amounts,
        allow_insecure_localhost = allow_insecure_localhost,
    ))
}

#[cfg(feature = "federation")]
fn dkg_probe_reveal(
    public_config: &FederationConfig,
    ceremony_id: FederationDkgCeremonyId,
    member_id: MemberId,
    keyset_spec: &FederationDkgKeysetSpec,
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
        ceremony_id,
        member_id,
        keyset_spec: keyset_spec.clone(),
        coefficient_commitments,
    })
}

#[cfg(feature = "federation")]
fn dkg_probe_secret_share_request(
    public_config: &FederationConfig,
    coefficients: &BTreeMap<MemberId, BTreeMap<Amount, Vec<BlsSecretKey>>>,
    ceremony_id: FederationDkgCeremonyId,
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
        ceremony_id,
        sender_id,
        receiver_id,
        secret_shares,
    ))
}

#[cfg(feature = "federation")]
fn write_signed_dkg_probe_request<T>(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
    receiver_id: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    payload: T,
    path: &Path,
    deterministic_test_signatures: bool,
    force: bool,
) -> Result<()>
where
    T: serde::Serialize,
{
    let request = match deterministic_test_signatures {
        true => FederationSignedRequest::new_signed_no_aux_rand(
            public_config.federation_id,
            receiver_id,
            sequence,
            kind,
            payload,
            private_config,
        )?,
        false => FederationSignedRequest::new_signed(
            public_config.federation_id,
            receiver_id,
            sequence,
            kind,
            payload,
            private_config,
        )?,
    };
    request.verify(public_config, receiver_id)?;
    write_json_file(path, &request, force)
}

#[cfg(feature = "federation")]
fn dkg_probe_request_path(receiver_dir: &Path, sender_id: MemberId, label: &str) -> PathBuf {
    receiver_dir.join(format!("from-{}-{label}.json", sender_id.get()))
}

#[cfg(feature = "federation")]
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
    deterministic_test_seed: Option<&str>,
) -> Result<String> {
    let member_label = member_id.get().to_string();
    let mnemonic = match deterministic_test_seed {
        Some(seed) => deterministic_test_mnemonic(seed, &[&member_label])?,
        None => Mnemonic::generate(12)
            .map_err(|err| anyhow!("failed to generate member mnemonic: {err}"))?,
    }
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
allow_development_payment_observation_policy = true
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

#[cfg(feature = "federation")]
fn write_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{json}\n"), force)
}

#[cfg(feature = "federation")]
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
FINAL_PUBLIC_CONFIG="$PUBLIC_CONFIG"
DKG_SETUP_PUBLIC_CONFIG={dkg_setup_public_config_path}
POSTGRES_SCHEMA_RESET_URL={postgres_schema_reset_url}
RESET_POSTGRES_SCHEMAS="${{RESET_POSTGRES_SCHEMAS:-0}}"
MEMBERS=({members})
PRIMARY_MEMBER="${{PRIMARY_MEMBER:-{primary_member}}}"
RESTART_MEMBER="${{RESTART_MEMBER:-{restart_member}}}"
QUARANTINE_OFFLINE_MEMBER="${{QUARANTINE_OFFLINE_MEMBER:-$RESTART_MEMBER}}"
CRASH_WINDOW_MEMBER="${{CRASH_WINDOW_MEMBER:-$RESTART_MEMBER}}"
LONG_PARTITION_MEMBER="${{LONG_PARTITION_MEMBER:-$RESTART_MEMBER}}"
EXERCISE_CRASH_WINDOW="${{EXERCISE_CRASH_WINDOW:-1}}"
STATUS_RETRIES="${{STATUS_RETRIES:-60}}"
STATUS_SLEEP_SECONDS="${{STATUS_SLEEP_SECONDS:-1}}"
CHECK_PROMETHEUS_METRICS="${{CHECK_PROMETHEUS_METRICS:-1}}"
PROBE_DKG_SETUP_ROUTES="${{PROBE_DKG_SETUP_ROUTES:-0}}"
DKG_ROUTE_PROBE_ONLY="${{DKG_ROUTE_PROBE_ONLY:-0}}"
USE_DKG_SETUP_CONFIGS=0
FEDERATION_CONSENSUS_BACKEND="${{FEDERATION_CONSENSUS_BACKEND:-}}"
FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${{FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-}}"
if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
  PROBE_DKG_SETUP_ROUTES=1
fi
SMOKE_QUOTE_RETRIES="${{SMOKE_QUOTE_RETRIES:-300}}"
SMOKE_QUOTE_SLEEP_MS="${{SMOKE_QUOTE_SLEEP_MS:-100}}"
SMOKE_OPERATION_TIMEOUT_SECONDS="${{SMOKE_OPERATION_TIMEOUT_SECONDS:-300}}"
STANDARD_SEED_OFFSET="${{STANDARD_SEED_OFFSET:-}}"
STANDARD_OPERATION_BATCHES="${{STANDARD_OPERATION_BATCHES:-2}}"
POST_CRASH_SEED_OFFSET="${{POST_CRASH_SEED_OFFSET:-}}"
POST_CRASH_OPERATION_BATCHES="${{POST_CRASH_OPERATION_BATCHES:-1}}"
LONG_PARTITION_SEED_OFFSET="${{LONG_PARTITION_SEED_OFFSET:-}}"
LONG_PARTITION_OPERATION_BATCHES="${{LONG_PARTITION_OPERATION_BATCHES:-4}}"
ALEPH_BFT_AUTO_ROTATION_MAX_ROUND="${{ALEPH_BFT_AUTO_ROTATION_MAX_ROUND:-}}"
ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION="${{ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION:-1}}"
ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES="${{ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES:-1}}"
PENDING_WALLET_DB="${{PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/pending-melt-wallet.sqlite}}"
FAILED_PENDING_WALLET_DB="${{FAILED_PENDING_WALLET_DB:-$(dirname "$PUBLIC_CONFIG")/failed-pending-melt-wallet.sqlite}}"
PIDS=()

if [[ -n "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" && -z "$FEDERATION_CONSENSUS_BACKEND" ]]; then
  FEDERATION_CONSENSUS_BACKEND="aleph_bft"
fi
if [[ "$FEDERATION_CONSENSUS_BACKEND" == "aleph_bft" && -z "$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS" ]]; then
  FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="$SMOKE_OPERATION_TIMEOUT_SECONDS"
fi
if [[ -z "$STANDARD_SEED_OFFSET" ]]; then
  if [[ -n "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" ]]; then
    STANDARD_SEED_OFFSET="$ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES"
  else
    STANDARD_SEED_OFFSET=0
  fi
fi

member_config() {{
  if [[ "$USE_DKG_SETUP_CONFIGS" == "1" ]]; then
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
  if [[ "$USE_DKG_SETUP_CONFIGS" == "1" ]]; then
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
  if [[ "$USE_DKG_SETUP_CONFIGS" == "1" ]]; then
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
  local mintd_env=()
  local mintd_env_count=0
  config="$(member_config "$member")"
  work_dir="$(member_work_dir "$member")"
  mkdir -p "$work_dir"
  if [[ "$CHECK_PROMETHEUS_METRICS" != "1" ]]; then
    mintd_env+=(CDK_MINTD_PROMETHEUS_ENABLED=false)
    mintd_env_count=$((mintd_env_count + 1))
  fi
  if [[ "$USE_DKG_SETUP_CONFIGS" == "1" ]]; then
    mintd_env+=(CDK_FEDERATION_DISABLE_PEER_SYNC=1)
    mintd_env_count=$((mintd_env_count + 1))
    mintd_env+=(CDK_MINTD_FEDERATION_CONSENSUS_BACKEND=in_memory)
    mintd_env_count=$((mintd_env_count + 1))
  elif [[ -n "$FEDERATION_CONSENSUS_BACKEND" ]]; then
    mintd_env+=(CDK_MINTD_FEDERATION_CONSENSUS_BACKEND="$FEDERATION_CONSENSUS_BACKEND")
    mintd_env_count=$((mintd_env_count + 1))
  fi
  if [[ -n "$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS" ]]; then
    mintd_env+=(CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS")
    mintd_env_count=$((mintd_env_count + 1))
  fi
  if [[ -n "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" ]]; then
    mintd_env+=(CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND="$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND")
    mintd_env_count=$((mintd_env_count + 1))
    mintd_env+=(CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT="$ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION")
    mintd_env_count=$((mintd_env_count + 1))
  fi
  if (( mintd_env_count == 0 )); then
    "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 &
  else
    env "${{mintd_env[@]}}" "$CDK_MINTD_BIN" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 &
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
  "$CDK_MINTD_BIN" federation status --public-config "$PUBLIC_CONFIG" --private-config "$private_config" --member "$member" --require-ready --require-session-outcome-quorum --require-no-digest-alerts --require-no-invalid-operation-alerts | awk '/^member / {{ print; exit }}'
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
    cdk_federation_pending_mempool_items \
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

aleph_bft_status_field() {{
  local line="$1"
  local key="$2"
  local field
  for field in $line; do
    case "$field" in
      "$key":*)
        printf '%s\n' "${{field#*:}}"
        return 0
        ;;
    esac
  done
  return 1
}}

run_smoke_operations() {{
  local has_private_config=0
  local timeout_args=(--operation-timeout-seconds "$SMOKE_OPERATION_TIMEOUT_SECONDS")
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
      "${{timeout_args[@]}}" \
      "$@"
    return
  fi
  "$CDK_MINTD_BIN" federation smoke-operations \
    --public-config "$PUBLIC_CONFIG" \
    --quote-retries "$SMOKE_QUOTE_RETRIES" \
    --quote-sleep-ms "$SMOKE_QUOTE_SLEEP_MS" \
    "${{timeout_args[@]}}" \
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

wait_for_aleph_bft_session_at_least() {{
  local member="$1"
  local target_session="$2"
  local work_dir
  work_dir="$(member_work_dir "$member")"
  for _ in $(seq 1 "$STATUS_RETRIES"); do
    local line
    local session
    if line="$(member_status_line "$member" 2>/dev/null)" && session="$(aleph_bft_status_field "$line" session)"; then
      if (( session >= target_session )); then
        return 0
      fi
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done
  printf 'member %s did not advance to AlephBFT session %s\n' "$member" "$target_session" >&2
  member_status_line "$member" >&2 || true
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  return 1
}}

wait_for_configured_aleph_bft_auto_rotation() {{
  if [[ -z "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" ]]; then
    return 0
  fi
  local member
  for member in "${{MEMBERS[@]}}"; do
    wait_for_aleph_bft_session_at_least "$member" "$ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION"
  done
  wait_for_converged
}}

run_configured_aleph_bft_auto_rotation_warmup() {{
  if [[ -z "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" ]]; then
    return 0
  fi
  run_smoke_operations \
    --scenario standard \
    --seed-offset 0 \
    --operation-batches "$ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES"
  wait_for_configured_aleph_bft_auto_rotation
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

validate_smoke_members() {{
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
  member_config "$CRASH_WINDOW_MEMBER" >/dev/null || {{
    printf 'unknown CRASH_WINDOW_MEMBER %s\n' "$CRASH_WINDOW_MEMBER" >&2
    return 1
  }}
  member_config "$LONG_PARTITION_MEMBER" >/dev/null || {{
    printf 'unknown LONG_PARTITION_MEMBER %s\n' "$LONG_PARTITION_MEMBER" >&2
    return 1
  }}
  if [[ "$PRIMARY_MEMBER" == "$LONG_PARTITION_MEMBER" ]]; then
    printf 'LONG_PARTITION_MEMBER must differ from PRIMARY_MEMBER for long-partition operation smoke\n' >&2
    return 1
  fi
}}

require_positive_integer() {{
  local name="$1"
  local value="$2"
  if ! [[ "$value" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s must be a positive integer\n' "$name" >&2
    return 1
  fi
}}

require_nonnegative_integer() {{
  local name="$1"
  local value="$2"
  if ! [[ "$value" =~ ^(0|[1-9][0-9]*)$ ]]; then
    printf '%s must be a non-negative integer\n' "$name" >&2
    return 1
  fi
}}

validate_smoke_settings() {{
  require_positive_integer STATUS_RETRIES "$STATUS_RETRIES" || return 1
  require_positive_integer STATUS_SLEEP_SECONDS "$STATUS_SLEEP_SECONDS" || return 1
  require_positive_integer SMOKE_QUOTE_RETRIES "$SMOKE_QUOTE_RETRIES" || return 1
  require_positive_integer SMOKE_QUOTE_SLEEP_MS "$SMOKE_QUOTE_SLEEP_MS" || return 1
  require_positive_integer SMOKE_OPERATION_TIMEOUT_SECONDS "$SMOKE_OPERATION_TIMEOUT_SECONDS" || return 1
  require_nonnegative_integer STANDARD_SEED_OFFSET "$STANDARD_SEED_OFFSET" || return 1
  require_positive_integer STANDARD_OPERATION_BATCHES "$STANDARD_OPERATION_BATCHES" || return 1
  require_positive_integer POST_CRASH_OPERATION_BATCHES "$POST_CRASH_OPERATION_BATCHES" || return 1
  require_positive_integer LONG_PARTITION_OPERATION_BATCHES "$LONG_PARTITION_OPERATION_BATCHES" || return 1
  if (( STANDARD_SEED_OFFSET > 255 )); then
    printf 'STANDARD_SEED_OFFSET must be 255 or lower\n' >&2
    return 1
  fi
  if (( STANDARD_SEED_OFFSET + STANDARD_OPERATION_BATCHES > 256 )); then
    printf 'STANDARD_SEED_OFFSET plus STANDARD_OPERATION_BATCHES must be 256 or lower\n' >&2
    return 1
  fi
  if [[ -z "$POST_CRASH_SEED_OFFSET" ]]; then
    POST_CRASH_SEED_OFFSET=$((STANDARD_SEED_OFFSET + STANDARD_OPERATION_BATCHES))
  fi
  require_nonnegative_integer POST_CRASH_SEED_OFFSET "$POST_CRASH_SEED_OFFSET" || return 1
  if (( POST_CRASH_SEED_OFFSET < STANDARD_SEED_OFFSET + STANDARD_OPERATION_BATCHES )); then
    printf 'POST_CRASH_SEED_OFFSET must be after the standard smoke seed range\n' >&2
    return 1
  fi
  if (( POST_CRASH_SEED_OFFSET + POST_CRASH_OPERATION_BATCHES > 256 )); then
    printf 'POST_CRASH_SEED_OFFSET plus POST_CRASH_OPERATION_BATCHES must be 256 or lower\n' >&2
    return 1
  fi
  if [[ -z "$LONG_PARTITION_SEED_OFFSET" ]]; then
    LONG_PARTITION_SEED_OFFSET=$((POST_CRASH_SEED_OFFSET + POST_CRASH_OPERATION_BATCHES))
  fi
  require_nonnegative_integer LONG_PARTITION_SEED_OFFSET "$LONG_PARTITION_SEED_OFFSET" || return 1
  if (( LONG_PARTITION_SEED_OFFSET < STANDARD_SEED_OFFSET + STANDARD_OPERATION_BATCHES )); then
    printf 'LONG_PARTITION_SEED_OFFSET must be after the standard smoke seed range\n' >&2
    return 1
  fi
  if [[ "$EXERCISE_CRASH_WINDOW" == "1" ]]; then
    if (( LONG_PARTITION_SEED_OFFSET < POST_CRASH_SEED_OFFSET + POST_CRASH_OPERATION_BATCHES && LONG_PARTITION_SEED_OFFSET + LONG_PARTITION_OPERATION_BATCHES > POST_CRASH_SEED_OFFSET )); then
      printf 'LONG_PARTITION_SEED_OFFSET plus LONG_PARTITION_OPERATION_BATCHES must not overlap the post-crash seed range\n' >&2
      return 1
    fi
  fi
  if (( LONG_PARTITION_SEED_OFFSET + LONG_PARTITION_OPERATION_BATCHES > 256 )); then
    printf 'LONG_PARTITION_SEED_OFFSET plus LONG_PARTITION_OPERATION_BATCHES must be 256 or lower\n' >&2
    return 1
  fi
  if [[ -n "$FEDERATION_CONSENSUS_BACKEND" && "$FEDERATION_CONSENSUS_BACKEND" != "in_memory" && "$FEDERATION_CONSENSUS_BACKEND" != "aleph_bft" ]]; then
    printf 'FEDERATION_CONSENSUS_BACKEND must be in_memory or aleph_bft\n' >&2
    return 1
  fi
  if [[ -n "$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS" ]]; then
    require_positive_integer FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS "$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS" || return 1
  fi
  if [[ -n "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" ]]; then
    require_positive_integer ALEPH_BFT_AUTO_ROTATION_MAX_ROUND "$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND" || return 1
    require_positive_integer ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION "$ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION" || return 1
    require_positive_integer ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES "$ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES" || return 1
    if [[ "$FEDERATION_CONSENSUS_BACKEND" != "aleph_bft" ]]; then
      printf 'ALEPH_BFT_AUTO_ROTATION_MAX_ROUND requires FEDERATION_CONSENSUS_BACKEND=aleph_bft\n' >&2
      return 1
    fi
    if (( ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES > 255 )); then
      printf 'ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES must be 255 or lower\n' >&2
      return 1
    fi
    if (( STANDARD_SEED_OFFSET < ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES )); then
      printf 'STANDARD_SEED_OFFSET must be after the AlephBFT auto-rotation warm-up seed range\n' >&2
      return 1
    fi
    if (( ALEPH_BFT_AUTO_ROTATION_MAX_ROUND > 65535 )); then
      printf 'ALEPH_BFT_AUTO_ROTATION_MAX_ROUND must be 65535 or lower\n' >&2
      return 1
    fi
  fi
}}

trap cleanup EXIT INT TERM

validate_smoke_members
validate_smoke_settings
if [[ "$PROBE_DKG_SETUP_ROUTES" == "1" ]]; then
  USE_DKG_SETUP_CONFIGS=1
  PUBLIC_CONFIG="$DKG_SETUP_PUBLIC_CONFIG"
fi
reset_member_work_dirs

for member in "${{MEMBERS[@]}}"; do
  start_member "$member"
done

probe_dkg_setup_routes
wait_for_dkg_setup_records
if [[ "$PROBE_DKG_SETUP_ROUTES" == "1" ]]; then
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
  if [[ "$DKG_ROUTE_PROBE_ONLY" == "1" ]]; then
    printf 'dkg setup route probe passed\n'
    exit 0
  fi
  for member in "${{MEMBERS[@]}}"; do
    stop_member "$member"
  done
  USE_DKG_SETUP_CONFIGS=0
  PUBLIC_CONFIG="$FINAL_PUBLIC_CONFIG"
  reset_member_work_dirs
  for member in "${{MEMBERS[@]}}"; do
    start_member "$member"
  done
fi

for member in "${{MEMBERS[@]}}"; do
  wait_for_status "$member"
  wait_for_dkg_status "$member"
done

wait_for_converged
wait_for_prometheus_metrics
run_configured_aleph_bft_auto_rotation_warmup
run_smoke_operations \
  --scenario standard \
  --seed-offset "$STANDARD_SEED_OFFSET" \
  --operation-batches "$STANDARD_OPERATION_BATCHES"
if [[ "$EXERCISE_CRASH_WINDOW" == "1" ]]; then
  stop_member "$CRASH_WINDOW_MEMBER"
  start_member "$CRASH_WINDOW_MEMBER"
  wait_for_status "$CRASH_WINDOW_MEMBER"
  wait_for_converged
  wait_for_session_outcome_quorum "$CRASH_WINDOW_MEMBER"
  run_smoke_operations \
    --scenario standard \
    --seed-offset "$POST_CRASH_SEED_OFFSET" \
    --operation-batches "$POST_CRASH_OPERATION_BATCHES"
  wait_for_converged
else
  wait_for_converged
fi

stop_member "$LONG_PARTITION_MEMBER"
run_smoke_operations \
  --scenario offline-member-operations \
  --seed-offset "$LONG_PARTITION_SEED_OFFSET" \
  --operation-batches "$LONG_PARTITION_OPERATION_BATCHES"
start_member "$LONG_PARTITION_MEMBER"
wait_for_status "$LONG_PARTITION_MEMBER"
wait_for_converged
wait_for_session_outcome_quorum "$LONG_PARTITION_MEMBER"

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

#[cfg(all(test, feature = "federation"))]
mod core_tests;

#[cfg(all(test, feature = "federation", feature = "fakewallet"))]
mod tests;
