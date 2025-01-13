//! Cdk mintd lib

use std::path::PathBuf;

use anyhow::anyhow;

pub mod cli;
pub mod config;
pub mod env_vars;
pub mod setup;

fn expand_path(path: &str) -> Option<PathBuf> {
    if path.starts_with('~') {
        if let Some(home_dir) = home::home_dir().as_mut() {
            let remainder = &path[2..];
            home_dir.push(remainder);
            let expanded_path = home_dir;
            Some(expanded_path.clone())
        } else {
            None
        }
    } else {
        Some(PathBuf::from(path))
    }
}

/// Work dir
pub fn work_dir() -> anyhow::Result<PathBuf> {
    let home_dir = home::home_dir().ok_or(anyhow!("Unknown home dir"))?;
    let dir = home_dir.join(".cdk-mintd");

    std::fs::create_dir_all(&dir)?;

    Ok(dir)
}

#[cfg(test)]
mod test {
    use std::env::current_dir;

    use super::*;

    #[test]
    fn example_is_parsed() {
        let config = config::Settings::new(Some(format!(
            "{}/example.config.toml",
            current_dir().expect("cwd").to_string_lossy()
        )));
        let cache = config.info.http_cache;

        assert_eq!(cache.ttl, Some(60));
        assert_eq!(cache.tti, Some(60));
    }
}
