//! Utility functions for interacting with cdk-ldk-node

use std::path::PathBuf;

use anyhow::Result;

/// Expand tilde paths to home directory
pub fn expand_path(path: &str) -> Result<PathBuf> {
    if path.starts_with('~') {
        let home = home::home_dir().ok_or(anyhow::anyhow!("Could not find home directory"))?;
        Ok(home.join(path.strip_prefix("~/").unwrap_or(&path[1..])))
    } else {
        Ok(PathBuf::from(path))
    }
}
