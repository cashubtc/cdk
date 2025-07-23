//! Utility functions for interacting with cdk-ldk-node

use std::path::PathBuf;

use anyhow::Result;

/// Expand tilde paths to home directory
pub fn expand_path(path: &str) -> Result<PathBuf> {
    if let Some(stripped) = path.strip_prefix('~') {
        let home = home::home_dir().ok_or(anyhow::anyhow!("Could not find home directory"))?;
        Ok(home.join(path.strip_prefix("~/").unwrap_or(stripped)))
    } else {
        Ok(PathBuf::from(path))
    }
}
