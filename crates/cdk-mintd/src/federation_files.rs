//! Federation files of the daemon: the protected readers and writers of the
//! setup ceremony, and the plain writers of local test fixtures.

#[cfg(any(test, feature = "federation-test-utils"))]
use std::path::Path;

#[cfg(any(test, feature = "federation-test-utils"))]
use anyhow::{bail, Context, Result};
pub(crate) use cdk_federation::setup::files::write_private_json_file;
#[cfg(all(any(test, feature = "federation-test-utils"), feature = "fakewallet"))]
pub(crate) use cdk_federation::setup::files::write_private_text_file;

#[cfg(any(test, feature = "federation-test-utils"))]
pub(crate) fn write_json_file<T>(path: &Path, value: &T, force: bool) -> Result<()>
where
    T: serde::Serialize + ?Sized,
{
    let json = serde_json::to_string_pretty(value)?;
    write_text_file(path, &format!("{json}\n"), force)
}

#[cfg(any(test, feature = "federation-test-utils"))]
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
