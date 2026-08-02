use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
#[cfg(test)]
use cdk_iroh::EndpointTicket;
use cdk_iroh::{DiscoveryMode, EndpointId, IrohConfig, IrohNode, RelayUrl, SecretKey};
use url::Url;
use zeroize::Zeroizing;

use crate::config::{Iroh, IrohDiscovery, Settings};

const DEFAULT_IROH_DIRECTORY: &str = "iroh";
const DEFAULT_SECRET_KEY_FILE: &str = "endpoint-secret";
const DEFAULT_ENDPOINT_TICKET_FILE: &str = "endpoint-ticket";
const SECRET_KEY_BYTES: usize = 32;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One persistent Iroh endpoint owned by the mint process.
#[derive(Clone)]
pub(crate) struct MintdIrohRuntime {
    pub(crate) node: IrohNode,
}

impl std::fmt::Debug for MintdIrohRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintdIrohRuntime")
            .field(
                "endpoint_id",
                &self.node.endpoint_id().fmt_short().to_string(),
            )
            .finish()
    }
}

pub(crate) async fn initialize(
    settings: &mut Settings,
    work_dir: &Path,
) -> Result<Option<MintdIrohRuntime>> {
    validate_listener_selection(settings)?;
    let Some(iroh) = settings.iroh.as_ref().filter(|iroh| iroh.enabled).cloned() else {
        return Ok(None);
    };

    let key_path = secret_key_path(&iroh, work_dir);
    let secret_key = load_or_generate_secret_key(&key_path, iroh.secret_key_file.is_none())?;
    let transport_config = transport_config(&iroh)?;
    let node = IrohNode::persistent(transport_config, secret_key)
        .await
        .context("failed to initialize persistent Iroh endpoint")?;

    let endpoint_url = format!("iroh://{}", node.endpoint_id());
    if settings.info.url == "iroh://auto" {
        settings.info.url.clone_from(&endpoint_url);
    } else {
        validate_configured_iroh_url(&settings.info.url, node.endpoint_id())?;
    }
    let ticket_path = endpoint_ticket_path(&iroh, work_dir);
    write_protected_replace(&ticket_path, node.endpoint_ticket().to_string().as_bytes())
        .context("failed to export current Iroh endpoint ticket")?;

    tracing::info!(
        endpoint = %node.endpoint_id(),
        url = %endpoint_url,
        discovery = ?iroh.discovery,
        "persistent Iroh mint endpoint ready"
    );
    Ok(Some(MintdIrohRuntime { node }))
}

pub(crate) fn validate_listener_selection(settings: &Settings) -> Result<()> {
    let iroh_enabled = settings.iroh.as_ref().is_some_and(|iroh| iroh.enabled);
    if !settings.info.http_enabled && !iroh_enabled {
        bail!("at least one public mint listener (HTTP or Iroh) must be enabled");
    }

    let public_scheme = Url::parse(&settings.info.url)
        .ok()
        .map(|url| url.scheme().to_owned());
    if public_scheme.as_deref() == Some("iroh") && !iroh_enabled {
        bail!("an iroh public mint URL requires the Iroh listener");
    }
    if !settings.info.http_enabled && public_scheme.as_deref() != Some("iroh") {
        bail!("Iroh-only operation requires an iroh public mint URL");
    }
    Ok(())
}

fn transport_config(iroh: &Iroh) -> Result<IrohConfig> {
    let relay_urls = iroh
        .relay_urls
        .iter()
        .map(|relay| RelayUrl::from_str(relay).context("invalid configured Iroh relay URL"))
        .collect::<Result<Vec<_>>>()?;
    let discovery = match iroh.discovery {
        IrohDiscovery::N0 => {
            if !relay_urls.is_empty() {
                bail!("relay_urls are only valid with custom Iroh discovery");
            }
            DiscoveryMode::N0
        }
        IrohDiscovery::Static => {
            if !relay_urls.is_empty() {
                bail!("relay_urls are only valid with custom Iroh discovery");
            }
            DiscoveryMode::Static
        }
        IrohDiscovery::Custom => {
            if relay_urls.is_empty() {
                bail!("custom Iroh discovery requires at least one relay URL");
            }
            DiscoveryMode::custom(relay_urls)
        }
    };
    Ok(IrohConfig {
        discovery,
        bind_addr: iroh.bind_addr,
        ..IrohConfig::default()
    })
}

fn validate_configured_iroh_url(value: &str, expected: EndpointId) -> Result<()> {
    let Ok(url) = Url::parse(value) else {
        return Ok(());
    };
    if url.scheme() != "iroh" {
        return Ok(());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("configured Iroh URL contains unsupported authority or suffix components");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("configured Iroh URL has no endpoint ID"))?;
    let configured = EndpointId::from_str(host)
        .context("configured Iroh URL contains an invalid endpoint ID")?;
    if configured != expected {
        bail!("configured Iroh URL endpoint ID does not match the protected mint endpoint key");
    }
    Ok(())
}

fn secret_key_path(iroh: &Iroh, work_dir: &Path) -> PathBuf {
    iroh.secret_key_file.clone().unwrap_or_else(|| {
        work_dir
            .join(DEFAULT_IROH_DIRECTORY)
            .join(DEFAULT_SECRET_KEY_FILE)
    })
}

fn endpoint_ticket_path(iroh: &Iroh, work_dir: &Path) -> PathBuf {
    iroh.endpoint_ticket_file.clone().unwrap_or_else(|| {
        work_dir
            .join(DEFAULT_IROH_DIRECTORY)
            .join(DEFAULT_ENDPOINT_TICKET_FILE)
    })
}

fn load_or_generate_secret_key(path: &Path, protect_default_parent: bool) -> Result<SecretKey> {
    match read_secret_key(path) {
        Ok(secret_key) => return Ok(secret_key),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == ErrorKind::NotFound) => {}
        Err(error) => return Err(error),
    }
    create_parent(path, protect_default_parent)?;
    let secret_key = SecretKey::generate();
    match create_secret_key(path, &secret_key) {
        Ok(()) => Ok(secret_key),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == ErrorKind::AlreadyExists) =>
        {
            read_secret_key(path)
        }
        Err(error) => Err(error),
    }
}

fn read_secret_key(path: &Path) -> Result<SecretKey> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!("protected Iroh endpoint secret must be a regular non-symlink file");
    }
    let mut file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    if opened_metadata.len() != SECRET_KEY_BYTES as u64 {
        bail!("protected Iroh endpoint secret must contain exactly 32 bytes");
    }
    validate_private_permissions(&opened_metadata)?;
    let mut bytes = Zeroizing::new([0_u8; SECRET_KEY_BYTES]);
    file.read_exact(&mut bytes[..])?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn create_parent(path: &Path, protect: bool) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let existed = parent.exists();
    std::fs::create_dir_all(parent)?;
    if protect && !existed {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn create_secret_key(path: &Path, secret_key: &SecretKey) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    restrict_private_permissions(&file)?;
    let bytes = Zeroizing::new(secret_key.to_bytes());
    file.write_all(bytes.as_ref())?;
    file.sync_all()?;
    Ok(())
}

fn write_protected_replace(path: &Path, contents: &[u8]) -> Result<()> {
    create_parent(path, false)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("iroh-endpoint-ticket");
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{file_name}.{}.{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        restrict_private_permissions(&file)?;
        file.write_all(contents)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn validate_private_permissions(metadata: &std::fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("protected Iroh endpoint secret permissions allow group or other access");
        }
    }
    Ok(())
}

fn restrict_private_permissions(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled_static() -> Iroh {
        Iroh {
            enabled: true,
            discovery: IrohDiscovery::Static,
            bind_addr: Some("127.0.0.1:0".parse().expect("loopback address")),
            ..Iroh::default()
        }
    }

    #[test]
    fn config_debug_redacts_paths_relays_and_tickets() {
        let mut iroh = enabled_static();
        iroh.secret_key_file = Some(PathBuf::from("sensitive-parent/endpoint-secret"));
        iroh.endpoint_ticket_file = Some(PathBuf::from("sensitive-parent/endpoint-ticket"));
        iroh.relay_urls = vec!["https://private-relay.invalid".to_string()];
        let rendered = format!("{iroh:?}");
        assert!(!rendered.contains("sensitive-parent"));
        assert!(!rendered.contains("private-relay"));
    }

    #[test]
    fn persistent_key_is_reloaded_and_protected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("iroh").join("endpoint-secret");
        let first = load_or_generate_secret_key(&path, true).expect("generate key");
        let second = load_or_generate_secret_key(&path, true).expect("reload key");
        assert_eq!(first.public(), second.public());
        assert_eq!(std::fs::metadata(&path).expect("metadata").len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(&path)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().expect("parent"))
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn corrupt_and_weak_keys_fail_closed() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("endpoint-secret");
        std::fs::write(&path, b"short").expect("corrupt key");
        assert!(load_or_generate_secret_key(&path, false).is_err());

        std::fs::write(&path, [7_u8; 32]).expect("key bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("weak permissions");
            assert!(load_or_generate_secret_key(&path, false).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn protected_key_reader_rejects_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("target");
        let link = directory.path().join("endpoint-secret");
        std::fs::write(&target, [9_u8; 32]).expect("target key");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .expect("target permissions");
        symlink(&target, &link).expect("key symlink");
        assert!(load_or_generate_secret_key(&link, false).is_err());
    }

    #[test]
    fn listener_selection_requires_one_matching_public_transport() {
        let mut settings = Settings::default();
        settings.info.http_enabled = false;
        assert!(validate_listener_selection(&settings).is_err());

        settings.iroh = Some(enabled_static());
        settings.info.url = "https://mint.invalid".to_string();
        assert!(validate_listener_selection(&settings).is_err());

        settings.info.url = "iroh://auto".to_string();
        assert!(validate_listener_selection(&settings).is_ok());

        let endpoint = SecretKey::generate().public();
        settings.info.url = format!("iroh://{endpoint}");
        assert!(validate_listener_selection(&settings).is_ok());
        assert!(validate_configured_iroh_url(&settings.info.url, endpoint).is_ok());
        assert!(
            validate_configured_iroh_url(&settings.info.url, SecretKey::generate().public())
                .is_err()
        );
    }

    #[tokio::test]
    async fn endpoint_identity_ticket_and_restart_are_stable() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut settings = Settings::default();
        settings.info.http_enabled = false;
        settings.info.url = "iroh://auto".to_string();
        settings.iroh = Some(enabled_static());

        let first = initialize(&mut settings, directory.path())
            .await
            .expect("first endpoint")
            .expect("enabled endpoint");
        let endpoint = first.node.endpoint_id();
        assert_eq!(first.node.endpoint_id(), endpoint);
        assert_eq!(settings.info.url, format!("iroh://{endpoint}"));
        let ticket_path = directory
            .path()
            .join(DEFAULT_IROH_DIRECTORY)
            .join(DEFAULT_ENDPOINT_TICKET_FILE);
        let ticket = std::fs::read_to_string(&ticket_path).expect("exported ticket");
        let parsed = EndpointTicket::from_str(ticket.trim()).expect("valid exported ticket");
        assert_eq!(parsed.endpoint_addr().id, endpoint);
        first.node.close().await;

        let restarted = initialize(&mut settings, directory.path())
            .await
            .expect("restarted endpoint")
            .expect("enabled endpoint");
        assert_eq!(restarted.node.endpoint_id(), endpoint);
        restarted.node.close().await;
    }
}
