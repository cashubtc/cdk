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
    generate_trusted_dealer_configs_with_options, FederationCheckpointPolicy, MemberId,
    ThresholdParams, TrustedDealerConfigBundle, TrustedDealerGenerationOptions,
    TrustedDealerKeysetSpec, TrustedDealerMember,
};
#[cfg(feature = "federation")]
use cdk_common::mint_url::MintUrl;
#[cfg(feature = "federation")]
use cdk_common::nuts::nut01::BlsSecretKey;
#[cfg(feature = "federation")]
use cdk_common::{Amount, CurrencyUnit, SecretKey};

#[cfg(feature = "federation")]
use crate::cli::{FederationTrustedDealerArgs, FederationTrustedDealerDatabaseEngine};
#[cfg(all(feature = "federation", feature = "fakewallet"))]
use crate::federation_files::{write_executable_text_file, write_private_text_file};
use crate::federation_files::{write_json_file, write_private_json_file, write_text_file};

#[cfg(feature = "federation")]
const PUBLIC_CONFIG_FILE: &str = "federation-public.json";
#[cfg(feature = "federation")]
const PRIVATE_CONFIG_FILE: &str = "federation-private.json";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_FILE: &str = "config.toml";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const MEMBER_CONFIG_TEMPLATE: &str = include_str!("../dev/federation/member-config.toml.in");
#[cfg(feature = "federation")]
const GENERATED_GITIGNORE_FILE: &str = ".gitignore";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_FILE: &str = "smoke-local-federation.sh";
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const SMOKE_SCRIPT_TEMPLATE: &str = include_str!("../dev/federation/smoke-local-federation.sh.in");
#[cfg(all(feature = "federation", feature = "fakewallet"))]
const POSTGRES_DATABASE_CONFIG_TEMPLATE: &str =
    include_str!("../dev/federation/postgres-database.toml.in");

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerCoreArtifacts {
    pub(crate) public_config_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerCoreMemberArtifacts>,
}

#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerCoreMemberArtifacts {
    pub(crate) private_config_path: PathBuf,
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
    pub(crate) smoke_script_path: PathBuf,
    pub(crate) member_artifacts: BTreeMap<MemberId, TrustedDealerMemberArtifacts>,
}

#[cfg(all(feature = "federation", feature = "fakewallet"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustedDealerMemberArtifacts {
    pub(crate) config_path: PathBuf,
    pub(crate) private_config_path: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub(crate) public_mint_url: String,
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
    for (member_id, member) in &artifacts.member_artifacts {
        println!(
            "Wrote member {} private config: {}",
            member_id,
            member.private_config_path.display()
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
    print_trusted_dealer_stage("setup.validate.done", setup_started_at);

    print_trusted_dealer_stage("setup.public-files.start", setup_started_at);
    let public_config_path = output_dir.join(PUBLIC_CONFIG_FILE);
    write_json_file(&public_config_path, &bundle.public_config, args.force)?;
    print_trusted_dealer_stage("setup.public-files.done", setup_started_at);

    print_trusted_dealer_stage("setup.member-files.start", setup_started_at);
    let mut core_member_artifacts = BTreeMap::new();
    for (member_id, private_config) in &bundle.private_configs {
        let member_dir = output_dir.join(format!("member-{}", member_id.get()));
        std::fs::create_dir_all(&member_dir)
            .with_context(|| format!("failed to create {}", member_dir.display()))?;
        let private_config_path = member_dir.join(PRIVATE_CONFIG_FILE);
        write_private_json_file(&private_config_path, private_config, args.force)?;

        core_member_artifacts.insert(
            *member_id,
            TrustedDealerCoreMemberArtifacts {
                private_config_path,
            },
        );
    }
    print_trusted_dealer_stage("setup.member-files.done", setup_started_at);

    Ok(TrustedDealerCoreGeneration {
        core_artifacts: TrustedDealerCoreArtifacts {
            public_config_path,
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
        let config_path = member_dir.join(MEMBER_CONFIG_FILE);
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

        member_artifacts.insert(
            *member_id,
            TrustedDealerMemberArtifacts {
                config_path,
                private_config_path: core_member.private_config_path.clone(),
                work_dir,
                public_mint_url: member_url,
                prometheus_url,
            },
        );
    }
    print_trusted_dealer_stage("setup.local-member-files.done", setup_started_at);

    print_trusted_dealer_stage("setup.smoke-script.start", setup_started_at);
    let smoke_script_path = generation.output_dir.join(SMOKE_SCRIPT_FILE);
    let smoke_script = local_smoke_script(&public_config_path, &member_artifacts, args)?;
    write_executable_text_file(&smoke_script_path, &smoke_script, args.force)?;
    print_trusted_dealer_stage("setup.smoke-script.done", setup_started_at);
    print_trusted_dealer_stage("setup.done", setup_started_at);

    Ok(TrustedDealerArtifacts {
        public_config_path,
        smoke_script_path,
        member_artifacts,
    })
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
    if args.amounts.contains(&0) {
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
    let secret_path = config_path.with_extension("mnemonic");
    write_private_text_file(&secret_path, &mnemonic, true)?;
    let mnemonic = format!("file:{}", path_to_string(&secret_path));
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
    let prometheus_config = if cfg!(feature = "prometheus") {
        format!("[prometheus]\nenabled = true\naddress = \"127.0.0.1\"\nport = {prometheus_port}\n")
    } else {
        String::new()
    };
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
            ("# @@PROMETHEUS_CONFIG@@\n", &prometheus_config),
            ("# @@DATABASE_CONFIG@@\n", database_config),
            ("@@LN_UNIT@@", &unit),
            ("@@SUPPORTED_UNIT@@", &unit),
            ("@@PUBLIC_CONFIG_PATH@@", &public_config_path),
            ("@@PRIVATE_CONFIG_PATH@@", &private_config_path),
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
            let secret_path = args
                .output_dir
                .join(format!("member-{member_id}-postgres-url"));
            write_private_text_file(&secret_path, &url, args.force)?;
            let url = toml_string(&format!("file:{}", path_to_string(&secret_path)))?;
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
    let config_cases = member_path_cases(member_artifacts, |member| &member.config_path)?;
    let work_dir_cases = member_path_cases(member_artifacts, |member| &member.work_dir)?;
    let private_config_cases =
        member_path_cases(member_artifacts, |member| &member.private_config_path)?;
    let public_mint_url_cases =
        member_string_cases(member_artifacts, |member| &member.public_mint_url);
    let prometheus_url_cases =
        member_string_cases(member_artifacts, |member| &member.prometheus_url);
    let public_config_path = shell_single_quote(&path_to_string(public_config_path));
    let postgres_schema_reset_url = postgres_schema_reset_url(args)
        .map(|url| shell_single_quote(&url))
        .unwrap_or_else(|| "''".to_string());
    let postgres_schema_reset_cases = postgres_schema_reset_cases(member_artifacts);
    let database_engine = match args.database_engine {
        FederationTrustedDealerDatabaseEngine::Sqlite => "sqlite",
        FederationTrustedDealerDatabaseEngine::Postgres => "postgres",
    };
    let postgres_secret_dir = shell_single_quote(&path_to_string(&args.output_dir));

    let primary_member = primary_member.get().to_string();
    let unit = shell_single_quote(&args.unit.to_lowercase());
    render_generated_template(
        SMOKE_SCRIPT_TEMPLATE,
        &[
            ("@@PUBLIC_CONFIG_PATH@@", &public_config_path),
            ("@@DATABASE_ENGINE@@", database_engine),
            ("@@POSTGRES_SECRET_DIR@@", &postgres_secret_dir),
            ("@@POSTGRES_SCHEMA_RESET_URL@@", &postgres_schema_reset_url),
            ("@@MEMBERS@@", &members),
            ("@@PRIMARY_MEMBER@@", &primary_member),
            ("@@UNIT@@", &unit),
            ("# @@CONFIG_CASES@@\n", &config_cases),
            ("# @@WORK_DIR_CASES@@\n", &work_dir_cases),
            ("# @@PRIVATE_CONFIG_CASES@@\n", &private_config_cases),
            ("# @@PUBLIC_MINT_URL_CASES@@\n", &public_mint_url_cases),
            ("# @@PROMETHEUS_URL_CASES@@\n", &prometheus_url_cases),
            (
                "# @@POSTGRES_SCHEMA_RESET_CASES@@\n",
                &postgres_schema_reset_cases,
            ),
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
