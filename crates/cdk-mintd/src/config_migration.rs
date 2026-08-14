use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::config::{Settings, Signatory};
use crate::secret::{SecretRef, SecretRefError};

const DEFAULT_SECRETS_DIRECTORY: &str = "cdk-mintd-secrets";
const TEMPORARY_FILE_ATTEMPTS: usize = 100;
static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
const RELEASED_V017_SIGNATORY_URL_ENV_VAR: &str = "CDK_MINTD_SIGNATORY_URL";
const RELEASED_V017_SIGNATORY_CERTS_ENV_VAR: &str = "CDK_MINTD_SIGNATORY_CERTS";
const RELEASED_V017_UNKNOWN_FIELDS: &[&str] = &[
    "info.signatory_certs",
    "info.signatory_url",
    #[cfg(feature = "management-rpc")]
    "mint_management_rpc.tls_dir_path",
];
#[cfg(feature = "management-rpc")]
const RELEASED_V017_MANAGEMENT_TLS_DIR_ENV_VAR: &str = "CDK_MINTD_MANAGEMENT_TLS_DIR_PATH";
#[cfg(feature = "redis")]
const ENV_CACHE_BACKEND: &str = "CDK_MINTD_CACHE_BACKEND";
#[cfg(feature = "redis")]
const ENV_CACHE_REDIS_URL: &str = "CDK_MINTD_CACHE_REDIS_URL";

#[derive(Default, Deserialize)]
#[serde(default)]
struct ReleasedV017Document {
    info: ReleasedV017Info,
    signatory: Option<CanonicalSignatoryTable>,
    mint_management_rpc: Option<ReleasedV017ManagementRpc>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ReleasedV017Info {
    signatory_url: Option<String>,
    signatory_certs: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CanonicalSignatoryTable {}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ReleasedV017ManagementRpc {
    tls_dir_path: Option<PathBuf>,
    tls_dir: Option<PathBuf>,
    allow_insecure: Option<bool>,
}

/// Result of converting a legacy mintd configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// Absolute path of the generated import document.
    pub output: PathBuf,
    /// Directory containing generated secret files, when any were needed.
    pub secrets_dir: Option<PathBuf>,
    /// Number of literal secrets copied into generated files.
    pub secret_files_written: usize,
}

#[derive(Debug, Clone, Copy)]
enum SecretNormalization {
    Opaque,
    Trim,
}

#[derive(Debug)]
struct SecretFile {
    path: PathBuf,
    value: String,
}

#[derive(Debug)]
struct StagedMigrationFile {
    kind: &'static str,
    destination: PathBuf,
    staged: PathBuf,
    backup: Option<PathBuf>,
    published: bool,
}

#[derive(Debug)]
struct MigrationSecrets {
    directory: PathBuf,
    manage_directory_permissions: bool,
    files: Vec<SecretFile>,
    protected_files: Vec<PathBuf>,
}

impl MigrationSecrets {
    fn new(directory: PathBuf, manage_directory_permissions: bool) -> Self {
        Self {
            directory,
            manage_directory_permissions,
            files: Vec::new(),
            protected_files: Vec::new(),
        }
    }

    fn add(&mut self, name: &str, value: &str) -> String {
        let path = self.directory.join(name);
        self.files.push(SecretFile {
            path: path.clone(),
            value: value.to_owned(),
        });
        SecretRef::File(path).to_string()
    }

    fn protect(&mut self, path: PathBuf) {
        if !self.protected_files.contains(&path) {
            self.protected_files.push(path);
        }
    }
}

/// Converts a legacy TOML document and its active `CDK_MINTD_*` overrides into
/// one database-importable TOML document.
///
/// Environment-backed secrets remain `env:` references. Literal secrets are
/// copied into owner-only files and replaced by absolute `file:` references.
/// The source document is never overwritten.
pub fn migrate_legacy_configuration(
    source: &Path,
    output: &Path,
    secrets_dir: Option<&Path>,
    legacy_seed_file: Option<&Path>,
    force: bool,
) -> Result<MigrationOutcome> {
    let source = source.canonicalize().with_context(|| {
        format!(
            "could not resolve legacy configuration {}",
            source.display()
        )
    })?;
    let output = absolute_path(output)?;

    let document = fs::read_to_string(&source)
        .with_context(|| format!("could not read legacy configuration {}", source.display()))?;
    let parse_context = format!("could not parse legacy configuration {}", source.display());
    let released_v017 = released_v017_document(&document).with_context(|| parse_context.clone())?;
    let mut effective = Settings::try_from_toml_allowing(&document, RELEASED_V017_UNKNOWN_FIELDS)
        .with_context(|| parse_context)?;
    effective = effective
        .from_env()
        .context("could not apply legacy environment overrides")?;

    let legacy_seed_file = match legacy_seed_file {
        Some(seed_file) => {
            let seed_file = existing_absolute_path(seed_file)?;
            crate::apply_seed_file(&mut effective, &seed_file)?;
            Some(seed_file)
        }
        None => None,
    };
    let seed_reference = legacy_seed_file
        .as_ref()
        .map(|seed_file| SecretRef::File(seed_file.clone()).to_string());

    apply_released_v017_compatibility(&mut effective, &released_v017)?;
    crate::config_service::prune_inactive_configuration(&mut effective);
    let mut migrated = effective.clone();
    let mut resolved = effective;

    let (secrets_dir, manage_directory_permissions) = match secrets_dir {
        Some(secrets_dir) => (absolute_path(secrets_dir)?, false),
        None => (
            output
                .parent()
                .unwrap_or(Path::new("/"))
                .join(DEFAULT_SECRETS_DIRECTORY),
            true,
        ),
    };
    let source_dir = source.parent().unwrap_or(Path::new("/"));
    let mut secrets = MigrationSecrets::new(secrets_dir, manage_directory_permissions);
    secrets.protect(source.clone());
    if let Some(seed_file) = legacy_seed_file {
        secrets.protect(seed_file);
    }

    externalize_optional_secret(
        &mut migrated.info.seed,
        &mut resolved.info.seed,
        "mint-seed",
        &[crate::env_vars::ENV_SEED],
        None,
        SecretNormalization::Opaque,
        source_dir,
        &mut secrets,
    )?;
    externalize_optional_secret(
        &mut migrated.info.mnemonic,
        &mut resolved.info.mnemonic,
        "mint-mnemonic",
        &[crate::env_vars::ENV_MNEMONIC],
        seed_reference.as_deref(),
        SecretNormalization::Trim,
        source_dir,
        &mut secrets,
    )?;

    if let (Some(migrated_postgres), Some(resolved_postgres)) = (
        migrated.database.postgres.as_mut(),
        resolved.database.postgres.as_mut(),
    ) {
        externalize_secret(
            &mut migrated_postgres.url,
            &mut resolved_postgres.url,
            "postgres-url",
            &[
                crate::env_vars::ENV_POSTGRES_URL,
                crate::env_vars::DATABASE_URL_ENV_VAR,
            ],
            None,
            SecretNormalization::Opaque,
            source_dir,
            &mut secrets,
        )?;
    }

    if let (Some(migrated_postgres), Some(resolved_postgres)) = (
        migrated
            .auth_database
            .as_mut()
            .and_then(|database| database.postgres.as_mut()),
        resolved
            .auth_database
            .as_mut()
            .and_then(|database| database.postgres.as_mut()),
    ) {
        externalize_secret(
            &mut migrated_postgres.url,
            &mut resolved_postgres.url,
            "auth-postgres-url",
            &[crate::env_vars::ENV_AUTH_POSTGRES_URL],
            None,
            SecretNormalization::Opaque,
            source_dir,
            &mut secrets,
        )?;
    }

    #[cfg(feature = "bdk")]
    if let (Some(migrated_bdk), Some(resolved_bdk)) = (migrated.bdk.as_mut(), resolved.bdk.as_mut())
    {
        externalize_optional_secret(
            &mut migrated_bdk.bitcoind_rpc_password,
            &mut resolved_bdk.bitcoind_rpc_password,
            "bdk-bitcoind-rpc-password",
            &[crate::env_vars::BDK_BITCOIND_RPC_PASSWORD_ENV_VAR],
            None,
            SecretNormalization::Opaque,
            source_dir,
            &mut secrets,
        )?;
        externalize_optional_secret(
            &mut migrated_bdk.mnemonic,
            &mut resolved_bdk.mnemonic,
            "bdk-mnemonic",
            &[crate::env_vars::BDK_MNEMONIC_ENV_VAR],
            seed_reference.as_deref(),
            SecretNormalization::Trim,
            source_dir,
            &mut secrets,
        )?;
    }

    #[cfg(feature = "ldk-node")]
    if let (Some(migrated_ldk), Some(resolved_ldk)) =
        (migrated.ldk_node.as_mut(), resolved.ldk_node.as_mut())
    {
        externalize_optional_secret(
            &mut migrated_ldk.bitcoind_rpc_password,
            &mut resolved_ldk.bitcoind_rpc_password,
            "ldk-node-bitcoind-rpc-password",
            &[crate::env_vars::LDK_NODE_BITCOIND_RPC_PASSWORD_ENV_VAR],
            None,
            SecretNormalization::Opaque,
            source_dir,
            &mut secrets,
        )?;
        externalize_optional_secret(
            &mut migrated_ldk.ldk_node_mnemonic,
            &mut resolved_ldk.ldk_node_mnemonic,
            "ldk-node-mnemonic",
            &[crate::env_vars::LDK_NODE_MNEMONIC_ENV_VAR],
            seed_reference.as_deref(),
            SecretNormalization::Trim,
            source_dir,
            &mut secrets,
        )?;
    }

    #[cfg(feature = "redis")]
    if let (
        cdk_axum::cache::Backend::Redis(migrated_redis),
        cdk_axum::cache::Backend::Redis(resolved_redis),
    ) = (
        &mut migrated.info.http_cache.backend,
        &mut resolved.info.http_cache.backend,
    ) {
        let connection_env_names = std::env::var(ENV_CACHE_BACKEND)
            .is_ok_and(|backend| backend.eq_ignore_ascii_case("redis"))
            .then_some([ENV_CACHE_REDIS_URL]);
        externalize_secret(
            &mut migrated_redis.connection_string,
            &mut resolved_redis.connection_string,
            "redis-connection-string",
            connection_env_names.as_ref().map_or(&[], |names| names),
            None,
            SecretNormalization::Opaque,
            source_dir,
            &mut secrets,
        )?;
        if let (Some(migrated_nodes), Some(resolved_nodes)) = (
            migrated_redis.cluster_nodes.as_mut(),
            resolved_redis.cluster_nodes.as_mut(),
        ) {
            for (index, (migrated_node, resolved_node)) in migrated_nodes
                .iter_mut()
                .zip(resolved_nodes.iter_mut())
                .enumerate()
            {
                externalize_secret(
                    migrated_node,
                    resolved_node,
                    &format!("redis-cluster-node-{}", index + 1),
                    &[],
                    None,
                    SecretNormalization::Opaque,
                    source_dir,
                    &mut secrets,
                )?;
            }
        }
    }

    crate::validate_settings(&resolved).context("legacy effective configuration is invalid")?;
    let migrated_document =
        toml::to_string_pretty(&migrated).context("could not serialize migrated configuration")?;
    Settings::try_from_toml(&migrated_document)
        .context("generated configuration did not round-trip through the TOML parser")?;

    prepare_write_destinations(&output, &secrets, force)?;
    if force {
        write_forced_migration(&output, &migrated_document, &secrets)?;
    } else {
        write_secret_files(&secrets, force)?;
        if let Err(error) = write_output(&output, &migrated_document, force) {
            remove_generated_secrets(&secrets);
            return Err(error);
        }
    }

    Ok(MigrationOutcome {
        output,
        secrets_dir: (!secrets.files.is_empty()).then_some(secrets.directory),
        secret_files_written: secrets.files.len(),
    })
}

fn released_v017_document(document: &str) -> Result<ReleasedV017Document> {
    Ok(config::Config::builder()
        .add_source(config::File::from_str(document, config::FileFormat::Toml))
        .build()?
        .try_deserialize()?)
}

fn apply_released_v017_compatibility(
    settings: &mut Settings,
    released: &ReleasedV017Document,
) -> Result<()> {
    apply_released_v017_signatory(settings, released)?;
    #[cfg(feature = "management-rpc")]
    apply_released_v017_management_rpc(settings, released);
    Ok(())
}

fn apply_released_v017_signatory(
    settings: &mut Settings,
    released: &ReleasedV017Document,
) -> Result<()> {
    let canonical_environment = [
        crate::env_vars::ENV_SIGNATORY_ENABLED,
        crate::env_vars::ENV_SIGNATORY_ADDRESS,
        crate::env_vars::ENV_SIGNATORY_PORT,
        crate::env_vars::ENV_SIGNATORY_TLS_DIR,
        crate::env_vars::ENV_SIGNATORY_ALLOW_INSECURE,
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some());
    if released.signatory.is_some() || canonical_environment {
        return Ok(());
    }

    let signatory_url = std::env::var(RELEASED_V017_SIGNATORY_URL_ENV_VAR)
        .ok()
        .or_else(|| released.info.signatory_url.clone());
    let Some(signatory_url) = signatory_url else {
        return Ok(());
    };
    let (address, port) = parse_released_v017_signatory_url(&signatory_url)?;
    let tls_dir = std::env::var(RELEASED_V017_SIGNATORY_CERTS_ENV_VAR)
        .ok()
        .or_else(|| released.info.signatory_certs.clone())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);

    settings.signatory = Some(Signatory {
        enabled: true,
        address,
        port,
        allow_insecure: tls_dir.is_none(),
        tls_dir,
    });
    // Released v0.17 selected the remote signatory before either local source.
    // Remove ignored local material so the new mutually-exclusive model keeps
    // the same effective signer.
    settings.info.seed = None;
    settings.info.mnemonic = None;
    Ok(())
}

fn parse_released_v017_signatory_url(url: &str) -> Result<(String, u16)> {
    let (scheme, authority_with_path) = url.split_once("://").ok_or_else(|| {
        anyhow!("released v0.17 signatory URL {url:?} must include an http:// or https:// scheme")
    })?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => bail!("released v0.17 signatory URL {url:?} uses unsupported scheme {scheme:?}"),
    };
    // Tonic accepted a root path in the released v0.17 setting, so preserve
    // compatibility with URLs such as `https://signatory.example/`.
    let authority = authority_with_path
        .strip_suffix('/')
        .unwrap_or(authority_with_path);
    if authority.is_empty()
        || authority.contains(['/', '?', '#', '@'])
        || authority.chars().any(char::is_whitespace)
    {
        bail!("released v0.17 signatory URL {url:?} is not a supported authority URL");
    }

    if let Some(ipv6) = authority.strip_prefix('[') {
        let end = ipv6.find(']').ok_or_else(|| {
            anyhow!("released v0.17 signatory URL {url:?} has an invalid IPv6 address")
        })?;
        let host = &ipv6[..end];
        if host.is_empty() {
            bail!("released v0.17 signatory URL {url:?} has an empty host");
        }
        let suffix = &ipv6[end + 1..];
        let port = match suffix.strip_prefix(':') {
            Some(port) => parse_released_v017_signatory_port(url, port)?,
            None if suffix.is_empty() => default_port,
            None => bail!("released v0.17 signatory URL {url:?} has an invalid authority"),
        };
        return Ok((format!("[{host}]"), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, parse_released_v017_signatory_port(url, port)?),
        None => (authority, default_port),
    };
    if host.is_empty() || host.contains(':') {
        bail!("released v0.17 signatory URL {url:?} has an invalid host");
    }
    Ok((host.to_owned(), port))
}

fn parse_released_v017_signatory_port(url: &str, port: &str) -> Result<u16> {
    port.parse::<u16>()
        .with_context(|| format!("released v0.17 signatory URL {url:?} has an invalid port"))
}

#[cfg(feature = "management-rpc")]
fn apply_released_v017_management_rpc(settings: &mut Settings, released: &ReleasedV017Document) {
    let Some(management_rpc) = settings.mint_management_rpc.as_mut() else {
        return;
    };
    let released_management = released.mint_management_rpc.as_ref();
    let canonical_tls_authored = released_management
        .and_then(|rpc| rpc.tls_dir.as_ref())
        .is_some()
        || std::env::var_os(crate::env_vars::ENV_MINT_MANAGEMENT_TLS_DIR).is_some();
    if !canonical_tls_authored {
        management_rpc.tls_dir = std::env::var(RELEASED_V017_MANAGEMENT_TLS_DIR_ENV_VAR)
            .ok()
            .map(PathBuf::from)
            .or_else(|| released_management.and_then(|rpc| rpc.tls_dir_path.clone()));
    }

    let canonical_security_authored = released_management
        .and_then(|rpc| rpc.allow_insecure)
        .is_some()
        || std::env::var_os(crate::env_vars::ENV_MINT_MANAGEMENT_ALLOW_INSECURE).is_some();
    let released_environment_enabled =
        std::env::var_os(crate::env_vars::ENV_MINT_MANAGEMENT_ENABLED).is_some()
            || std::env::var_os(crate::env_vars::ENV_MINT_MANAGEMENT_ENABLED_LEGACY).is_some();
    let released_table = released_management
        .is_some_and(|rpc| rpc.tls_dir.is_none() && rpc.allow_insecure.is_none());
    if management_rpc.enabled
        && management_rpc.tls_dir.is_none()
        && !canonical_security_authored
        && (released_table || released_environment_enabled)
    {
        // Released v0.17 fell back to plaintext when the TLS directory did not
        // exist. Preserve that behavior explicitly in the migrated document.
        management_rpc.allow_insecure = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn externalize_optional_secret(
    migrated: &mut Option<String>,
    resolved: &mut Option<String>,
    file_name: &str,
    env_names: &[&str],
    preferred_reference: Option<&str>,
    normalization: SecretNormalization,
    source_dir: &Path,
    secrets: &mut MigrationSecrets,
) -> Result<()> {
    match (migrated.as_mut(), resolved.as_mut()) {
        (Some(migrated), Some(resolved)) => externalize_secret(
            migrated,
            resolved,
            file_name,
            env_names,
            preferred_reference,
            normalization,
            source_dir,
            secrets,
        ),
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn externalize_secret(
    migrated: &mut String,
    resolved: &mut String,
    file_name: &str,
    env_names: &[&str],
    preferred_reference: Option<&str>,
    normalization: SecretNormalization,
    source_dir: &Path,
    secrets: &mut MigrationSecrets,
) -> Result<()> {
    if let Some(reference) = preferred_reference {
        *migrated = reference.to_owned();
        normalize_secret(resolved, normalization);
        return Ok(());
    }

    if let Some(name) = env_names.iter().find(|name| std::env::var(name).is_ok()) {
        if resolved.is_empty() {
            bail!("environment-backed secret {name} is empty");
        }
        *migrated = SecretRef::Environment((*name).to_owned()).to_string();
        normalize_secret(resolved, normalization);
        return Ok(());
    }

    match SecretRef::parse_relative_to(migrated, source_dir) {
        Ok(reference) => {
            let canonical = reference.to_string();
            *resolved = reference.resolve().map_err(|error| match &error {
                crate::secret::SecretResolveError::Environment { name } => {
                    anyhow!("could not resolve {file_name} from environment {name}")
                }
                crate::secret::SecretResolveError::File { path, source } => {
                    anyhow!(
                        "could not resolve {file_name} from {}: {source}",
                        path.display()
                    )
                }
            })?;
            if resolved.is_empty() {
                bail!("secret reference {canonical} resolved to an empty value");
            }
            if let SecretRef::File(path) = &reference {
                secrets.protect(path.canonicalize().with_context(|| {
                    format!(
                        "could not resolve referenced secret file {}",
                        path.display()
                    )
                })?);
            }
            *migrated = canonical;
            normalize_secret(resolved, normalization);
            return Ok(());
        }
        Err(SecretRefError::EmptyEnvironmentName) => {
            bail!("empty environment secret reference for {file_name}")
        }
        Err(SecretRefError::EmptyFilePath) => {
            bail!("empty file secret reference for {file_name}")
        }
        Err(SecretRefError::Literal | SecretRefError::RelativeFilePath { .. }) => {}
    }

    if migrated.is_empty() {
        return Ok(());
    }

    normalize_secret(resolved, normalization);
    *migrated = secrets.add(file_name, migrated);
    Ok(())
}

fn normalize_secret(value: &mut String, normalization: SecretNormalization) {
    if matches!(normalization, SecretNormalization::Trim) {
        *value = value.trim().to_owned();
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()
            .context("could not determine current directory")?
            .join(path))
    }
}

fn existing_absolute_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("could not resolve {}", path.display()))
}

fn prepare_write_destinations(
    output: &Path,
    secrets: &MigrationSecrets,
    force: bool,
) -> Result<()> {
    let output_destination = (
        "migration output",
        output,
        resolve_destination(output, "migration output")?,
    );
    ensure_distinct_write_destinations(
        std::slice::from_ref(&output_destination),
        &secrets.protected_files,
    )?;
    ensure_replaceable(output, force, "migration output")?;

    if !secrets.files.is_empty() {
        prepare_secrets_directory(&secrets.directory, secrets.manage_directory_permissions)?;
    }

    let mut destinations = Vec::with_capacity(secrets.files.len() + 1);
    destinations.push(output_destination);
    for secret in &secrets.files {
        destinations.push((
            "secret file",
            secret.path.as_path(),
            resolve_destination(&secret.path, "secret file")?,
        ));
    }

    ensure_distinct_write_destinations(&destinations, &secrets.protected_files)?;
    for (kind, path, _) in destinations.into_iter().skip(1) {
        ensure_replaceable(path, force, kind)?;
    }

    Ok(())
}

fn resolve_destination(path: &Path, kind: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{kind} {} has no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("{kind} {} is not a file path", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("could not create {kind} directory {}", parent.display()))?;
    let parent = parent
        .canonicalize()
        .with_context(|| format!("could not resolve {kind} directory {}", parent.display()))?;
    Ok(parent.join(file_name))
}

fn ensure_distinct_write_destinations(
    destinations: &[(&str, &Path, PathBuf)],
    protected_files: &[PathBuf],
) -> Result<()> {
    for (kind, path, resolved) in destinations {
        for protected in protected_files {
            if resolved == protected || same_file(path, protected)? {
                bail!(
                    "{kind} {} must differ from input file {}",
                    path.display(),
                    protected.display()
                );
            }
        }
    }

    for (index, (kind, path, resolved)) in destinations.iter().enumerate() {
        for (other_kind, other_path, other_resolved) in destinations.iter().skip(index + 1) {
            if resolved == other_resolved || same_file(path, other_path)? {
                bail!(
                    "{kind} {} must differ from {other_kind} {}",
                    path.display(),
                    other_path.display()
                );
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    if !left.exists() || !right.exists() {
        return Ok(false);
    }
    let left = fs::metadata(left)
        .with_context(|| format!("could not inspect source {}", left.display()))?;
    let right = fs::metadata(right)
        .with_context(|| format!("could not inspect output {}", right.display()))?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(not(unix))]
fn same_file(_left: &Path, _right: &Path) -> Result<bool> {
    Ok(false)
}

fn ensure_replaceable(path: &Path, force: bool, kind: &str) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            bail!("{kind} {} must not be a symbolic link", path.display());
        }
        if !metadata.is_file() {
            bail!("{kind} {} must be a regular file", path.display());
        }
        if has_multiple_hard_links(&metadata) {
            bail!(
                "{kind} {} must not have multiple hard links",
                path.display()
            );
        }
        if !force {
            bail!(
                "{kind} {} already exists; pass --force to overwrite it",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn has_multiple_hard_links(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink() > 1
}

#[cfg(not(unix))]
fn has_multiple_hard_links(_metadata: &fs::Metadata) -> bool {
    false
}

fn prepare_secrets_directory(path: &Path, manage_permissions: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_existing_secrets_directory(path, &metadata)?;
            if manage_permissions {
                set_directory_permissions(path)?;
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                anyhow!(
                    "secrets directory {} has no parent directory",
                    path.display()
                )
            })?;
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create secrets directory parent {}",
                    parent.display()
                )
            })?;

            match create_secret_directory(path) {
                Ok(()) => set_directory_permissions(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(path).with_context(|| {
                        format!("could not inspect secrets directory {}", path.display())
                    })?;
                    validate_existing_secrets_directory(path, &metadata)?;
                    if manage_permissions {
                        set_directory_permissions(path)?;
                    }
                    Ok(())
                }
                Err(error) => Err(error).with_context(|| {
                    format!("could not create secrets directory {}", path.display())
                }),
            }
        }
        Err(error) => Err(error)
            .with_context(|| format!("could not inspect secrets directory {}", path.display())),
    }
}

fn validate_existing_secrets_directory(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if metadata.file_type().is_symlink() {
        bail!(
            "secrets directory {} must not be a symbolic link",
            path.display()
        );
    }
    if !metadata.is_dir() {
        bail!("secrets directory {} is not a directory", path.display());
    }
    Ok(())
}

fn create_secret_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        builder.mode(0o700);
    }
    builder.create(path)
}

fn write_forced_migration(output: &Path, document: &str, secrets: &MigrationSecrets) -> Result<()> {
    let mut staged = Vec::with_capacity(secrets.files.len() + 1);
    for secret in &secrets.files {
        match stage_migration_file("secret file", &secret.path, &secret.value, true) {
            Ok(file) => staged.push(file),
            Err(error) => {
                remove_staged_files(&staged);
                return Err(error);
            }
        }
    }
    match stage_migration_file("migration output", output, document, false) {
        Ok(file) => staged.push(file),
        Err(error) => {
            remove_staged_files(&staged);
            return Err(error);
        }
    }

    commit_staged_files(&mut staged)
}

fn stage_migration_file(
    kind: &'static str,
    destination: &Path,
    contents: &str,
    secret: bool,
) -> Result<StagedMigrationFile> {
    let (staged, mut file) = create_temporary_sibling(destination, "staged", secret)?;
    let write_result = (|| -> Result<()> {
        file.write_all(contents.as_bytes())
            .with_context(|| format!("could not stage {kind} {}", destination.display()))?;
        if secret {
            set_secret_file_permissions(&staged)?;
        }
        file.sync_all()
            .with_context(|| format!("could not sync staged {kind} {}", destination.display()))?;
        Ok(())
    })();
    drop(file);

    if let Err(error) = write_result {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }

    Ok(StagedMigrationFile {
        kind,
        destination: destination.to_owned(),
        staged,
        backup: None,
        published: false,
    })
}

fn create_temporary_sibling(
    destination: &Path,
    role: &str,
    secret: bool,
) -> Result<(PathBuf, File)> {
    let parent = destination.parent().ok_or_else(|| {
        anyhow!(
            "migration destination {} has no parent directory",
            destination.display()
        )
    })?;

    for _ in 0..TEMPORARY_FILE_ATTEMPTS {
        let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".cdk-mintd-migration-{role}-{}-{counter}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        if secret {
            set_secret_creation_mode(&mut options);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "could not create temporary file beside {}",
                        destination.display()
                    )
                });
            }
        }
    }

    bail!(
        "could not allocate a temporary file beside {}",
        destination.display()
    )
}

fn unused_temporary_sibling(destination: &Path, role: &str) -> Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        anyhow!(
            "migration destination {} has no parent directory",
            destination.display()
        )
    })?;

    for _ in 0..TEMPORARY_FILE_ATTEMPTS {
        let counter = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".cdk-mintd-migration-{role}-{}-{counter}",
            std::process::id()
        ));
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(path),
            Ok(_) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "could not inspect temporary path beside {}",
                        destination.display()
                    )
                });
            }
        }
    }

    bail!(
        "could not allocate a temporary path beside {}",
        destination.display()
    )
}

fn commit_staged_files(staged: &mut [StagedMigrationFile]) -> Result<()> {
    if let Err(error) = backup_existing_files(staged) {
        let rollback_errors = rollback_staged_files(staged);
        remove_staged_files(staged);
        return Err(with_rollback_context(error, rollback_errors));
    }

    for index in 0..staged.len() {
        let file = &staged[index];
        if let Err(error) = fs::rename(&file.staged, &file.destination).with_context(|| {
            format!(
                "could not publish {} {}",
                file.kind,
                file.destination.display()
            )
        }) {
            let rollback_errors = rollback_staged_files(staged);
            remove_staged_files(staged);
            return Err(with_rollback_context(error, rollback_errors));
        }
        staged[index].published = true;
    }

    remove_backup_files(staged)
}

fn backup_existing_files(staged: &mut [StagedMigrationFile]) -> Result<()> {
    for file in staged {
        match fs::symlink_metadata(&file.destination) {
            Ok(_) => {
                ensure_replaceable(&file.destination, true, file.kind)?;
                let backup = unused_temporary_sibling(&file.destination, "backup")?;
                fs::rename(&file.destination, &backup).with_context(|| {
                    format!(
                        "could not back up {} {}",
                        file.kind,
                        file.destination.display()
                    )
                })?;
                file.backup = Some(backup);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "could not inspect {} {} before publishing",
                        file.kind,
                        file.destination.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn rollback_staged_files(staged: &mut [StagedMigrationFile]) -> Vec<String> {
    let mut errors = Vec::new();
    for file in staged.iter_mut().rev() {
        if file.published {
            match fs::remove_file(&file.destination) {
                Ok(()) => file.published = false,
                Err(error) => {
                    errors.push(format!(
                        "could not remove newly published {} {}: {error}",
                        file.kind,
                        file.destination.display()
                    ));
                    continue;
                }
            }
        }
        if let Some(backup) = file.backup.take() {
            if let Err(error) = fs::rename(&backup, &file.destination) {
                errors.push(format!(
                    "could not restore {} {} from {}: {error}",
                    file.kind,
                    file.destination.display(),
                    backup.display()
                ));
                file.backup = Some(backup);
            }
        }
    }
    errors
}

fn with_rollback_context(error: anyhow::Error, rollback_errors: Vec<String>) -> anyhow::Error {
    if rollback_errors.is_empty() {
        error
    } else {
        anyhow!(
            "{error:#}; migration rollback also failed: {}",
            rollback_errors.join("; ")
        )
    }
}

fn remove_staged_files(staged: &[StagedMigrationFile]) {
    for file in staged {
        let _ = fs::remove_file(&file.staged);
    }
}

fn remove_backup_files(staged: &mut [StagedMigrationFile]) -> Result<()> {
    for file in staged {
        if let Some(backup) = file.backup.take() {
            if let Err(error) = fs::remove_file(&backup) {
                file.backup = Some(backup.clone());
                return Err(error).with_context(|| {
                    format!(
                        "migration files were published, but the backup {} could not be removed",
                        backup.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn write_secret_files(secrets: &MigrationSecrets, force: bool) -> Result<()> {
    if secrets.files.is_empty() {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(&secrets.directory).with_context(|| {
        format!(
            "could not inspect secrets directory {}",
            secrets.directory.display()
        )
    })?;
    validate_existing_secrets_directory(&secrets.directory, &metadata)?;

    let mut written = Vec::new();
    for secret in &secrets.files {
        let result = write_secret_file(secret, force);
        match result {
            Ok(()) => written.push(secret.path.clone()),
            Err(error) => {
                if !force {
                    for path in written {
                        let _ = fs::remove_file(path);
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

fn write_secret_file(secret: &SecretFile, force: bool) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    set_secret_creation_mode(&mut options);
    let mut file = options
        .open(&secret.path)
        .with_context(|| format!("could not create secret file {}", secret.path.display()))?;
    let write_result = (|| -> Result<()> {
        file.write_all(secret.value.as_bytes())
            .with_context(|| format!("could not write secret file {}", secret.path.display()))?;
        set_secret_file_permissions(&secret.path)
    })();
    drop(file);

    if write_result.is_err() && !force {
        // The successful create-new open above establishes that this
        // invocation created the path. Never remove it when opening failed,
        // because another writer may have won the race after preflight.
        let _ = fs::remove_file(&secret.path);
    }
    write_result
}

fn remove_generated_secrets(secrets: &MigrationSecrets) {
    for secret in &secrets.files {
        let _ = fs::remove_file(&secret.path);
    }
}

fn write_output(path: &Path, document: &str, force: bool) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "migration output {} has no parent directory",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)
        .with_context(|| format!("could not create output directory {}", parent.display()))?;

    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("could not create migration output {}", path.display()))?;
    if let Err(error) = file
        .write_all(document.as_bytes())
        .with_context(|| format!("could not write migration output {}", path.display()))
    {
        if !force {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(unix)]
fn set_secret_creation_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_secret_creation_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("could not secure secrets directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_secret_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not secure secret file {}", path.display()))
}

#[cfg(not(unix))]
fn set_secret_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    struct MintdEnvironment {
        saved: Vec<(OsString, OsString)>,
    }

    impl MintdEnvironment {
        fn cleared() -> Self {
            let saved = std::env::vars_os()
                .filter(|(name, _)| name.to_string_lossy().starts_with("CDK_MINTD_"))
                .collect::<Vec<_>>();
            for (name, _) in &saved {
                std::env::remove_var(name);
            }
            Self { saved }
        }
    }

    impl Drop for MintdEnvironment {
        fn drop(&mut self) {
            for (name, _) in std::env::vars_os()
                .filter(|(name, _)| name.to_string_lossy().starts_with("CDK_MINTD_"))
            {
                std::env::remove_var(name);
            }
            for (name, value) in &self.saved {
                std::env::set_var(name, value);
            }
        }
    }

    fn legacy_document(mnemonic: &str) -> String {
        format!(
            r#"
[info]
mnemonic = "{mnemonic}"

[payment_backend]
backend = "fakewallet"

[fake_wallet]

[database]
engine = "sqlite"
"#
        )
    }

    fn released_v017_remote_signatory_document(url: &str, certs: Option<&str>) -> String {
        let certs = certs
            .map(|certs| format!("signatory_certs = \"{certs}\""))
            .unwrap_or_default();
        format!(
            r#"
[info]
signatory_url = "{url}"
{certs}

[payment_backend]
backend = "fakewallet"

[fake_wallet]

[database]
engine = "sqlite"
"#
        )
    }

    fn migration_paths(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let directory = crate::test_utils::unique_temp_path(name);
        fs::create_dir_all(&directory).expect("create migration test directory");
        let source = directory.join("legacy.toml");
        let output = directory.join("migrated.toml");
        (directory, source, output)
    }

    #[test]
    fn released_v017_signatory_url_accepts_root_paths() {
        assert_eq!(
            parse_released_v017_signatory_url("http://127.0.0.1:10009/")
                .expect("parse HTTP URL with root path"),
            ("127.0.0.1".to_owned(), 10009)
        );
        assert_eq!(
            parse_released_v017_signatory_url("https://signatory.example/")
                .expect("parse HTTPS URL with root path"),
            ("signatory.example".to_owned(), 443)
        );
    }

    #[test]
    fn migration_rejects_unknown_fields_outside_released_allowlist() {
        let (directory, source, output) = migration_paths("migrate_reject_unknown");
        fs::write(
            &source,
            r#"
[info]
listen_por = 8085
signatory_url = "http://127.0.0.1:10009"
"#,
        )
        .expect("write misspelled legacy config");

        let error = migrate_legacy_configuration(&source, &output, None, None, false)
            .expect_err("migration must reject unknown fields");
        let message = format!("{error:#}");
        assert!(message.contains("info.listen_por"));
        assert!(!output.exists());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_materializes_operational_env_and_preserves_env_secrets() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_env_secret");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write legacy config");
        std::env::set_var(crate::env_vars::ENV_LISTEN_PORT, "8123");
        std::env::set_var(crate::env_vars::ENV_MNEMONIC, TEST_MNEMONIC);

        let outcome = migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate legacy config");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");

        assert_eq!(settings.info.listen_port, 8123);
        assert_eq!(
            settings.info.mnemonic.as_deref(),
            Some("env:CDK_MINTD_MNEMONIC")
        );
        assert!(!migrated.contains(TEST_MNEMONIC));
        crate::config_service::ConfigurationService::validate_document(&migrated)
            .expect("validate migrated document");
        assert_eq!(outcome.secret_files_written, 0);
        assert!(outcome.secrets_dir.is_none());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_maps_released_v017_remote_signatory_fields() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_released_v017_remote_signatory");
        let seed_file = directory.join("legacy-seed");
        fs::write(
            &source,
            released_v017_remote_signatory_document(
                "https://signatory.example:15061/",
                Some("/run/cdk/signatory-tls"),
            ),
        )
        .expect("write released v0.17 config");
        fs::write(&seed_file, TEST_MNEMONIC).expect("write released v0.17 seed file");

        migrate_legacy_configuration(&source, &output, None, Some(&seed_file), false)
            .expect("migrate released v0.17 remote signatory with ignored seed file");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let signatory = settings.signatory.expect("migrated signatory");

        assert!(signatory.enabled);
        assert_eq!(signatory.address, "signatory.example");
        assert_eq!(signatory.port, 15061);
        assert_eq!(
            signatory.tls_dir,
            Some(PathBuf::from("/run/cdk/signatory-tls"))
        );
        assert!(!signatory.allow_insecure);
        assert!(settings.info.seed.is_none());
        assert!(settings.info.mnemonic.is_none());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_maps_released_v017_remote_signatory_environment() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) =
            migration_paths("migrate_released_v017_remote_signatory_env");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write released v0.17 config");
        std::env::set_var(
            RELEASED_V017_SIGNATORY_URL_ENV_VAR,
            "http://127.0.0.1:15062",
        );

        migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate released v0.17 signatory environment");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let signatory = settings.signatory.expect("migrated signatory");

        assert!(signatory.enabled);
        assert_eq!(signatory.address, "127.0.0.1");
        assert_eq!(signatory.port, 15062);
        assert!(signatory.tls_dir.is_none());
        assert!(signatory.allow_insecure);
        assert!(settings.info.mnemonic.is_none());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(all(feature = "fakewallet", feature = "management-rpc"))]
    #[test]
    fn migration_maps_released_v017_management_rpc_tls() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_released_v017_management_tls");
        let document = format!(
            r#"
{}

[mint_management_rpc]
enabled = true
address = "127.0.0.1"
port = 18086
tls_dir_path = "/run/cdk/management-tls"
"#,
            legacy_document(TEST_MNEMONIC)
        );
        fs::write(&source, document).expect("write released v0.17 config");

        migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate released v0.17 management RPC");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let management = settings
            .mint_management_rpc
            .expect("migrated management RPC");

        assert!(management.enabled);
        assert_eq!(
            management.tls_dir,
            Some(PathBuf::from("/run/cdk/management-tls"))
        );
        assert!(!management.allow_insecure);

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(all(feature = "fakewallet", feature = "management-rpc"))]
    #[test]
    fn migration_preserves_released_v017_management_rpc_insecure_fallback() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) =
            migration_paths("migrate_released_v017_management_insecure");
        let document = format!(
            r#"
{}

[mint_management_rpc]
enabled = true
"#,
            legacy_document(TEST_MNEMONIC)
        );
        fs::write(&source, document).expect("write released v0.17 config");

        migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate released v0.17 insecure management RPC");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let management = settings
            .mint_management_rpc
            .expect("migrated management RPC");

        assert!(management.enabled);
        assert!(management.tls_dir.is_none());
        assert!(management.allow_insecure);

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(all(feature = "fakewallet", feature = "management-rpc"))]
    #[test]
    fn migration_maps_released_v017_management_rpc_tls_environment() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) =
            migration_paths("migrate_released_v017_management_tls_env");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write released v0.17 config");
        std::env::set_var(crate::env_vars::ENV_MINT_MANAGEMENT_ENABLED, "true");
        std::env::set_var(
            RELEASED_V017_MANAGEMENT_TLS_DIR_ENV_VAR,
            "/run/cdk/management-tls-from-env",
        );

        migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate released v0.17 management RPC environment");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let management = settings
            .mint_management_rpc
            .expect("migrated management RPC");

        assert!(management.enabled);
        assert_eq!(
            management.tls_dir,
            Some(PathBuf::from("/run/cdk/management-tls-from-env"))
        );
        assert!(!management.allow_insecure);

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_extracts_literal_secrets_into_owner_only_files() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_literal_secret");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write legacy config");

        let outcome = migrate_legacy_configuration(&source, &output, None, None, false)
            .expect("migrate legacy config");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let secret_path = directory
            .join(DEFAULT_SECRETS_DIRECTORY)
            .join("mint-mnemonic");

        assert!(!migrated.contains(TEST_MNEMONIC));
        assert!(migrated.contains(&format!("file:{}", secret_path.display())));
        assert_eq!(
            fs::read_to_string(&secret_path).expect("read extracted mnemonic"),
            TEST_MNEMONIC
        );
        assert_eq!(outcome.secret_files_written, 1);
        assert_eq!(outcome.secrets_dir.as_deref(), secret_path.parent());
        crate::config_service::ConfigurationService::validate_document(&migrated)
            .expect("validate migrated document");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(secret_path.parent().expect("secret parent"))
                    .expect("secrets directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&secret_path)
                    .expect("secret metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[test]
    fn secret_cleanup_preserves_destination_created_by_another_writer() {
        let directory = crate::test_utils::unique_temp_path("migrate_secret_write_race");
        fs::create_dir(&directory).expect("create secrets directory");
        let mut secrets = MigrationSecrets::new(directory.clone(), false);
        secrets.add("first", "first secret");
        secrets.add("second", "our second secret");
        let first = directory.join("first");
        let second = directory.join("second");
        fs::write(&second, "other writer").expect("create competing secret");

        let error =
            write_secret_files(&secrets, false).expect_err("existing second secret must fail");

        assert!(error.to_string().contains("could not create secret file"));
        assert!(!first.exists());
        assert_eq!(
            fs::read_to_string(&second).expect("read competing secret"),
            "other writer"
        );

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn forced_migration_replaces_existing_output_and_secrets() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_force_replace");
        let secrets_dir = directory.join(DEFAULT_SECRETS_DIRECTORY);
        let secret_path = secrets_dir.join("mint-mnemonic");
        fs::create_dir(&secrets_dir).expect("create secrets directory");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write legacy config");
        fs::write(&output, "old output").expect("write old output");
        fs::write(&secret_path, "old mnemonic").expect("write old secret");

        migrate_legacy_configuration(&source, &output, None, None, true)
            .expect("replace existing migration files");

        let migrated = fs::read_to_string(&output).expect("read migrated output");
        assert!(migrated.contains(&format!("file:{}", secret_path.display())));
        assert_eq!(
            fs::read_to_string(&secret_path).expect("read replaced secret"),
            TEST_MNEMONIC
        );
        for path in fs::read_dir(&directory)
            .expect("read migration directory")
            .chain(fs::read_dir(&secrets_dir).expect("read secrets directory"))
            .map(|entry| entry.expect("directory entry").path())
        {
            assert!(
                !path
                    .file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .starts_with(".cdk-mintd-migration-"),
                "temporary migration file remained at {}",
                path.display()
            );
        }

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[test]
    fn forced_publish_rolls_back_after_partial_rename() {
        let directory = crate::test_utils::unique_temp_path("migrate_publish_rollback");
        fs::create_dir(&directory).expect("create migration test directory");
        let first = directory.join("first");
        let second = directory.join("second");
        fs::write(&first, "old first").expect("write first destination");
        fs::write(&second, "old second").expect("write second destination");

        let mut staged = vec![
            stage_migration_file("test file", &first, "new first", false)
                .expect("stage first file"),
            stage_migration_file("test file", &second, "new second", false)
                .expect("stage second file"),
        ];
        fs::remove_file(&staged[1].staged).expect("remove second staged file");

        let error = commit_staged_files(&mut staged).expect_err("publishing should fail");
        assert!(error.to_string().contains("could not publish test file"));
        assert_eq!(
            fs::read_to_string(&first).expect("read restored first destination"),
            "old first"
        );
        assert_eq!(
            fs::read_to_string(&second).expect("read restored second destination"),
            "old second"
        );

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(all(feature = "fakewallet", unix))]
    #[test]
    fn migration_preserves_existing_secrets_directory_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_existing_secret_permissions");
        let secrets_dir = directory.join("shared-secrets");
        fs::create_dir(&secrets_dir).expect("create existing secrets directory");
        fs::set_permissions(&secrets_dir, fs::Permissions::from_mode(0o750))
            .expect("set existing secrets directory permissions");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write legacy config");

        migrate_legacy_configuration(&source, &output, Some(&secrets_dir), None, false)
            .expect("migrate into existing secrets directory");

        assert_eq!(
            fs::metadata(&secrets_dir)
                .expect("existing secrets directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(
            fs::metadata(secrets_dir.join("mint-mnemonic"))
                .expect("secret metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_preserves_legacy_seed_file_as_an_absolute_reference() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_seed_file");
        let seed_file = directory.join("seed.txt");
        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write legacy config");
        fs::write(&seed_file, TEST_MNEMONIC).expect("write legacy seed file");

        let outcome = migrate_legacy_configuration(&source, &output, None, Some(&seed_file), false)
            .expect("migrate legacy seed file");
        let migrated = fs::read_to_string(&output).expect("read migrated config");
        let settings = Settings::try_from_toml(&migrated).expect("parse migrated config");
        let canonical_seed_file = seed_file.canonicalize().expect("canonical seed path");

        let expected_reference = format!("file:{}", canonical_seed_file.display());
        assert_eq!(
            settings.info.mnemonic.as_deref(),
            Some(expected_reference.as_str())
        );
        assert_eq!(outcome.secret_files_written, 0);
        assert!(outcome.secrets_dir.is_none());
        crate::config_service::ConfigurationService::validate_document(&migrated)
            .expect("validate migrated document");

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_refuses_to_overwrite_source_or_output_by_default() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, output) = migration_paths("migrate_no_overwrite");
        let document = legacy_document(TEST_MNEMONIC);
        fs::write(&source, &document).expect("write legacy config");
        fs::write(&output, "existing").expect("write existing output");

        let output_error = migrate_legacy_configuration(&source, &output, None, None, false)
            .expect_err("existing output should fail");
        assert!(output_error.to_string().contains("already exists"));

        let source_error = migrate_legacy_configuration(&source, &source, None, None, true)
            .expect_err("source overwrite should fail");
        assert!(source_error.to_string().contains("must differ"));

        let aliased_source = directory.join("missing").join("..").join("legacy.toml");
        let aliased_source_error =
            migrate_legacy_configuration(&source, &aliased_source, None, None, true)
                .expect_err("lexically aliased source overwrite should fail");
        assert!(aliased_source_error.to_string().contains("must differ"));

        #[cfg(unix)]
        {
            let hard_link = directory.join("legacy-hard-link.toml");
            fs::hard_link(&source, &hard_link).expect("create source hard link");
            let hard_link_error =
                migrate_legacy_configuration(&source, &hard_link, None, None, true)
                    .expect_err("source hard-link overwrite should fail");
            assert!(hard_link_error.to_string().contains("must differ"));
        }

        assert_eq!(
            fs::read_to_string(&source).expect("read preserved source"),
            document
        );

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_rejects_destinations_that_alias_input_secrets_or_each_other() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let (directory, source, _) = migration_paths("migrate_destination_aliases");
        let referenced_secret = directory.join("existing-mnemonic");
        let referenced_document =
            legacy_document(&SecretRef::File(PathBuf::from("existing-mnemonic")).to_string());
        fs::write(&source, referenced_document).expect("write referenced-secret config");
        fs::write(&referenced_secret, TEST_MNEMONIC).expect("write referenced secret");

        let input_alias_error =
            migrate_legacy_configuration(&source, &referenced_secret, None, None, true)
                .expect_err("output aliasing an input secret should fail");
        assert!(input_alias_error.to_string().contains("must differ"));
        assert_eq!(
            fs::read_to_string(&referenced_secret).expect("read preserved referenced secret"),
            TEST_MNEMONIC
        );

        fs::write(&source, legacy_document(TEST_MNEMONIC)).expect("write literal-secret config");
        let colliding_output = directory.join("mint-mnemonic");
        let destination_alias_error =
            migrate_legacy_configuration(&source, &colliding_output, Some(&directory), None, false)
                .expect_err("output aliasing a generated secret should fail");
        assert!(destination_alias_error.to_string().contains("must differ"));
        assert!(!colliding_output.exists());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn migration_rejects_generated_secret_that_aliases_source() {
        let _env_lock = crate::test_utils::env_lock();
        let _environment = MintdEnvironment::cleared();
        let directory = crate::test_utils::unique_temp_path("migrate_secret_source_alias");
        fs::create_dir_all(&directory).expect("create migration test directory");
        let source = directory.join("mint-mnemonic");
        let output = directory.join("migrated.toml");
        let document = legacy_document(TEST_MNEMONIC);
        fs::write(&source, &document).expect("write legacy config");

        let error = migrate_legacy_configuration(&source, &output, Some(&directory), None, true)
            .expect_err("generated secret aliasing the source should fail");
        assert!(error.to_string().contains("must differ"));
        assert_eq!(
            fs::read_to_string(&source).expect("read preserved source"),
            document
        );
        assert!(!output.exists());

        fs::remove_dir_all(directory).expect("remove migration test directory");
    }
}
