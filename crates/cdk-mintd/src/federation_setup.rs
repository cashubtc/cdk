#[cfg(feature = "federation")]
use std::collections::BTreeMap;
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use std::collections::BTreeSet;
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
    generate_trusted_dealer_configs_with_options, FederatedKeyset, FederationCheckpointPolicy,
    FederationConfig, FederationDkgCeremonyId, FederationDkgCommitmentRequest,
    FederationDkgCommitmentRound, FederationDkgKeysetSpec, FederationDkgParticipantCommitment,
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
use crate::federation_files::{
    write_executable_text_file, write_json_file, write_private_json_file, write_private_text_file,
    write_text_file,
};

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
const MEMBER_CONFIG_TEMPLATE: &str = include_str!("../dev/federation/member-config.toml.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const DKG_SETUP_MEMBER_CONFIG_FILE: &str = "dkg-setup-config.toml";
#[cfg(feature = "federation")]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_FILE: &str = "smoke-local-federation.sh";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_TEMPLATE: &str = include_str!("../dev/federation/smoke-local-federation.sh.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_MEMBER_CONFIG_FILE: &str = "dkg-live-config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const FROST_BDK_MEMBER_CONFIG_FILE: &str = "frost-bdk-config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const FROST_BDK_MEMBER_CONFIG_TEMPLATE: &str =
    include_str!("../dev/federation/frost-bdk-member-config.toml.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_CONFIG_TEMPLATE: &str = include_str!("../dev/federation/live-dkg-config.toml.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_FINAL_CONFIG_DIR: &str = "dkg-final";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_SMOKE_SCRIPT_FILE: &str = "smoke-live-dkg-federation.sh";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const LIVE_DKG_SMOKE_SCRIPT_TEMPLATE: &str =
    include_str!("../dev/federation/smoke-live-dkg-federation.sh.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const POSTGRES_DATABASE_CONFIG_TEMPLATE: &str =
    include_str!("../dev/federation/postgres-database.toml.in");
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
    pub(crate) live_dkg_smoke_script_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerMemberArtifacts {
    pub(crate) config_path: PathBuf,
    pub(crate) dkg_setup_config_path: PathBuf,
    pub(crate) live_dkg_config_path: PathBuf,
    pub(crate) frost_bdk_config_path: PathBuf,
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
        println!(
            "Run live DKG federation smoke test: {}",
            artifacts.live_dkg_smoke_script_path.display()
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
            checkpoint_policy: FederationCheckpointPolicy::new(
                args.checkpoint_interval_journal_operations,
                args.checkpoint_interval_finalized_items,
            )
            .context("invalid federation checkpoint policy")?,
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
        write_private_json_file(&private_config_path, private_config, args.force)?;
        let dkg_setup_private_config =
            dkg_setup_private_config(private_config, &dkg_setup_public_config)?;
        let dkg_setup_private_config_path = member_dir.join(DKG_SETUP_PRIVATE_CONFIG_FILE);
        write_private_json_file(
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
        let frost_bdk_config_path = member_dir.join(FROST_BDK_MEMBER_CONFIG_FILE);
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
        write_private_text_file(&config_path, &config_toml, args.force)?;
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
        write_private_text_file(&dkg_setup_config_path, &dkg_setup_config_toml, args.force)?;
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
        write_private_text_file(&live_dkg_config_path, &live_dkg_config_toml, args.force)?;
        let frost_bdk_config_toml = member_frost_bdk_config_toml(
            member_id,
            &args.host,
            &member_url,
            &federation_api_url,
            listen_port,
            prometheus_port,
            &live_dkg_final_config_dir.join(PUBLIC_CONFIG_FILE),
            &live_dkg_final_config_dir.join(PRIVATE_CONFIG_FILE),
            &frost_bdk_config_path,
            &live_dkg_work_dir,
            &database_config,
            args.deterministic_test_seed.as_deref(),
        )?;
        write_private_text_file(&frost_bdk_config_path, &frost_bdk_config_toml, args.force)?;

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                dkg_setup_config_path,
                live_dkg_config_path,
                frost_bdk_config_path,
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
    let live_dkg_smoke_script_path = generation.output_dir.join(LIVE_DKG_SMOKE_SCRIPT_FILE);
    let live_dkg_smoke_script =
        local_live_dkg_smoke_script(&dkg_setup_public_config_path, &member_artifacts, args)?;
    write_executable_text_file(
        &live_dkg_smoke_script_path,
        &live_dkg_smoke_script,
        args.force,
    )?;
    print_trusted_dealer_stage("setup.smoke-script.done", setup_started_at);
    print_trusted_dealer_stage("setup.done", setup_started_at);

    Ok(TrustedDealerArtifacts {
        public_config_path,
        dkg_setup_public_config_path,
        smoke_script_path,
        live_dkg_smoke_script_path,
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
    let mut final_public_config = FederationConfig::new(
        public_config.setup_authorization,
        public_config.threshold_params,
        public_config.members.clone(),
        vec![route_probe_keyset],
    )
    .context("generated DKG route-probe final public config is invalid")?;
    final_public_config.payment_observation_policy =
        public_config.payment_observation_policy.clone();
    final_public_config.checkpoint_policy = public_config.checkpoint_policy;
    final_public_config
        .validate()
        .context("generated DKG route-probe payment policy is invalid")?;
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

    let ceremony_id = toml_string(&ceremony_id.to_string())?;
    let unit = toml_string(&keyset_spec.unit.to_string().to_lowercase())?;
    let active = keyset_spec.active.to_string();
    let input_fee_ppk = keyset_spec.input_fee_ppk.to_string();
    let allow_insecure_localhost = allow_insecure_localhost.to_string();
    render_generated_template(
        LIVE_DKG_CONFIG_TEMPLATE,
        &[
            ("@@CEREMONY_ID@@", &ceremony_id),
            ("@@UNIT@@", &unit),
            ("@@ACTIVE@@", &active),
            ("@@INPUT_FEE_PPK@@", &input_fee_ppk),
            ("# @@EXPIRY@@\n", &expiry),
            ("@@AMOUNTS@@", &amounts),
            ("@@ALLOW_INSECURE_LOCALHOST@@", &allow_insecure_localhost),
        ],
    )
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
    write_private_json_file(path, &request, force)
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

    let work_dir = toml_string(&work_dir)?;
    let config_path = toml_string(&config_path)?;
    let mint_url = toml_string(mint_url)?;
    let listen_host = toml_string(listen_host)?;
    let listen_port = listen_port.to_string();
    let mnemonic = toml_string(&mnemonic)?;
    let name = toml_string(&format!("CDK federation member {}", member_id.get()))?;
    let description = toml_string(&format!(
        "Local fake-wallet federation member {} at {}",
        member_id.get(),
        federation_url
    ))?;
    let prometheus_port = prometheus_port.to_string();
    let unit = toml_string(&unit)?;
    let public_config_path = toml_string(&public_config_path)?;
    let private_config_path = toml_string(&private_config_path)?;
    render_generated_template(
        MEMBER_CONFIG_TEMPLATE,
        &[
            ("@@WORK_DIR@@", &work_dir),
            ("@@CONFIG_PATH@@", &config_path),
            ("@@MINT_URL@@", &mint_url),
            ("@@LISTEN_HOST@@", &listen_host),
            ("@@LISTEN_PORT@@", &listen_port),
            ("@@MNEMONIC@@", &mnemonic),
            ("@@NAME@@", &name),
            ("@@DESCRIPTION@@", &description),
            ("@@PROMETHEUS_PORT@@", &prometheus_port),
            ("# @@DATABASE_CONFIG@@\n", database_config),
            ("@@LN_UNIT@@", &unit),
            ("@@SUPPORTED_UNIT@@", &unit),
            ("@@PUBLIC_CONFIG_PATH@@", &public_config_path),
            ("@@PRIVATE_CONFIG_PATH@@", &private_config_path),
        ],
    )
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[allow(clippy::too_many_arguments)]
fn member_frost_bdk_config_toml(
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
    let description = format!(
        "Local federated BDK member {} at {}",
        member_id.get(),
        federation_url
    );
    render_generated_template(
        FROST_BDK_MEMBER_CONFIG_TEMPLATE,
        &[
            ("@@WORK_DIR@@", &toml_string(&path_to_string(work_dir))?),
            (
                "@@CONFIG_PATH@@",
                &toml_string(&path_to_string(config_path))?,
            ),
            ("@@MINT_URL@@", &toml_string(mint_url)?),
            ("@@LISTEN_HOST@@", &toml_string(listen_host)?),
            ("@@LISTEN_PORT@@", &listen_port.to_string()),
            ("@@MNEMONIC@@", &toml_string(&mnemonic)?),
            (
                "@@NAME@@",
                &toml_string(&format!("CDK FROST federation member {}", member_id.get()))?,
            ),
            ("@@DESCRIPTION@@", &toml_string(&description)?),
            ("@@PROMETHEUS_PORT@@", &prometheus_port.to_string()),
            ("# @@DATABASE_CONFIG@@\n", database_config),
            (
                "@@PUBLIC_CONFIG_PATH@@",
                &toml_string(&path_to_string(public_config_path))?,
            ),
            (
                "@@PRIVATE_CONFIG_PATH@@",
                &toml_string(&path_to_string(private_config_path))?,
            ),
        ],
    )
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
            let url = toml_string(&url)?;
            render_generated_template(POSTGRES_DATABASE_CONFIG_TEMPLATE, &[("@@URL@@", &url)])
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

    let primary_member = primary_member.get().to_string();
    let restart_member = restart_member.get().to_string();
    render_generated_template(
        SMOKE_SCRIPT_TEMPLATE,
        &[
            ("@@PUBLIC_CONFIG_PATH@@", &public_config_path),
            (
                "@@DKG_SETUP_PUBLIC_CONFIG_PATH@@",
                &dkg_setup_public_config_path,
            ),
            ("@@POSTGRES_SCHEMA_RESET_URL@@", &postgres_schema_reset_url),
            ("@@MEMBERS@@", &members),
            ("@@PRIMARY_MEMBER@@", &primary_member),
            ("@@RESTART_MEMBER@@", &restart_member),
            ("# @@CONFIG_CASES@@\n", &config_cases),
            ("# @@DKG_SETUP_CONFIG_CASES@@\n", &dkg_setup_config_cases),
            ("# @@WORK_DIR_CASES@@\n", &work_dir_cases),
            (
                "# @@DKG_SETUP_WORK_DIR_CASES@@\n",
                &dkg_setup_work_dir_cases,
            ),
            ("# @@PRIVATE_CONFIG_CASES@@\n", &private_config_cases),
            (
                "# @@DKG_SETUP_PRIVATE_CONFIG_CASES@@\n",
                &dkg_setup_private_config_cases,
            ),
            (
                "# @@DKG_SETUP_REQUEST_DIR_CASES@@\n",
                &dkg_setup_request_dir_cases,
            ),
            ("# @@PUBLIC_MINT_URL_CASES@@\n", &public_mint_url_cases),
            ("# @@FEDERATION_URL_CASES@@\n", &federation_url_cases),
            ("# @@PROMETHEUS_URL_CASES@@\n", &prometheus_url_cases),
            (
                "# @@POSTGRES_SCHEMA_RESET_CASES@@\n",
                &postgres_schema_reset_cases,
            ),
        ],
    )
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn local_live_dkg_smoke_script(
    setup_public_config_path: &Path,
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
        .ok_or_else(|| anyhow!("live DKG smoke needs at least one member"))?;
    let interrupted_member = member_artifacts.keys().nth(1).unwrap_or(primary_member);
    let held_member = member_artifacts
        .keys()
        .next_back()
        .unwrap_or(primary_member);
    let primary_final_public_config = member_artifacts
        .get(primary_member)
        .ok_or_else(|| anyhow!("missing primary member live DKG artifacts"))?
        .live_dkg_final_config_dir
        .join(PUBLIC_CONFIG_FILE);
    let config_cases = member_path_cases(member_artifacts, |member| &member.live_dkg_config_path)?;
    let frost_bdk_config_cases =
        member_path_cases(member_artifacts, |member| &member.frost_bdk_config_path)?;
    let work_dir_cases = member_path_cases(member_artifacts, |member| &member.live_dkg_work_dir)?;
    let setup_private_config_cases = member_path_cases(member_artifacts, |member| {
        &member.dkg_setup_private_config_path
    })?;
    let final_config_dir_cases =
        member_path_cases(member_artifacts, |member| &member.live_dkg_final_config_dir)?;
    let public_mint_url_cases =
        member_string_cases(member_artifacts, |member| &member.public_mint_url);
    let prometheus_url_cases =
        member_string_cases(member_artifacts, |member| &member.prometheus_url);
    let setup_public_config_path = shell_single_quote(&path_to_string(setup_public_config_path));
    let primary_final_public_config =
        shell_single_quote(&path_to_string(&primary_final_public_config));
    let postgres_schema_reset_url = postgres_schema_reset_url(args)
        .map(|url| shell_single_quote(&url))
        .unwrap_or_else(|| "''".to_string());
    let postgres_schema_reset_cases = postgres_schema_reset_cases(member_artifacts);
    let unit = args.unit.to_lowercase();

    let primary_member = primary_member.get().to_string();
    let interrupted_member = interrupted_member.get().to_string();
    let held_member = held_member.get().to_string();
    let member_count = member_artifacts.len().to_string();
    let signature_threshold = args.threshold.to_string();
    let unit = shell_single_quote(&unit);
    render_generated_template(
        LIVE_DKG_SMOKE_SCRIPT_TEMPLATE,
        &[
            ("@@SETUP_PUBLIC_CONFIG_PATH@@", &setup_public_config_path),
            (
                "@@PRIMARY_FINAL_PUBLIC_CONFIG@@",
                &primary_final_public_config,
            ),
            ("@@POSTGRES_SCHEMA_RESET_URL@@", &postgres_schema_reset_url),
            ("@@MEMBERS@@", &members),
            ("@@PRIMARY_MEMBER@@", &primary_member),
            ("@@INTERRUPTED_MEMBER@@", &interrupted_member),
            ("@@HELD_MEMBER@@", &held_member),
            ("# @@CONFIG_CASES@@\n", &config_cases),
            ("# @@FROST_BDK_CONFIG_CASES@@\n", &frost_bdk_config_cases),
            ("# @@WORK_DIR_CASES@@\n", &work_dir_cases),
            (
                "# @@SETUP_PRIVATE_CONFIG_CASES@@\n",
                &setup_private_config_cases,
            ),
            ("# @@FINAL_CONFIG_DIR_CASES@@\n", &final_config_dir_cases),
            ("# @@PUBLIC_MINT_URL_CASES@@\n", &public_mint_url_cases),
            ("# @@PROMETHEUS_URL_CASES@@\n", &prometheus_url_cases),
            (
                "# @@POSTGRES_SCHEMA_RESET_CASES@@\n",
                &postgres_schema_reset_cases,
            ),
            ("@@MEMBER_COUNT@@", &member_count),
            ("@@THRESHOLD@@", &signature_threshold),
            ("@@UNIT@@", &unit),
        ],
    )
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

#[cfg(all(feature = "federation", feature = "fakewallet"))]
fn render_generated_template(template: &str, substitutions: &[(&str, &str)]) -> Result<String> {
    let mut markers = BTreeSet::new();
    let mut template_without_markers = template.to_string();
    for (marker, _) in substitutions {
        if !markers.insert(*marker) {
            bail!("duplicate generated template marker {marker}");
        }
        let occurrences = template.matches(marker).count();
        if occurrences != 1 {
            bail!("generated template marker {marker} occurs {occurrences} times instead of once");
        }
        template_without_markers = template_without_markers.replace(marker, "");
    }
    if template_without_markers.contains("@@") {
        bail!("generated template contains an unknown marker");
    }

    let mut rendered = template.to_string();
    for (marker, value) in substitutions {
        rendered = rendered.replace(marker, value);
    }
    Ok(rendered)
}

#[cfg(all(test, feature = "federation"))]
mod core_tests;

#[cfg(all(test, feature = "federation", feature = "fakewallet"))]
mod tests;
