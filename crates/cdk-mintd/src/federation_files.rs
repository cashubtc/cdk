use std::io::Write;
use std::path::Path;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};

#[cfg(unix)]
static PRIVATE_FILE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{json}\n"), force)
}

pub(crate) fn write_private_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_private_text_file(path, &format!("{json}\n"), force)
}

pub(crate) fn write_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
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

pub(crate) fn write_private_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
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

    #[cfg(unix)]
    if force {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("federation-private");
        let sequence = PRIVATE_FILE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary_path = path.with_file_name(format!(
            ".{file_name}.{}.{sequence}.tmp",
            std::process::id()
        ));
        let result = (|| {
            write_private_file_contents(&temporary_path, contents, true)?;
            std::fs::rename(&temporary_path, path).with_context(|| {
                format!(
                    "failed to atomically replace private file {}",
                    path.display()
                )
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary_path);
        }
        return result;
    }

    write_private_file_contents(path, contents, !force)
}

fn write_private_file_contents(path: &Path, contents: &str, create_new: bool) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    match create_new {
        true => {
            options.create_new(true);
        }
        false => {
            options.create(true).truncate(true);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create private file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict private file {}", path.display()))?;
    }
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write private file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync private file {}", path.display()))
}

#[cfg(any(test, feature = "federation-test-utils"))]
pub(crate) fn write_executable_text_file(path: &Path, contents: &str, force: bool) -> Result<()> {
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
