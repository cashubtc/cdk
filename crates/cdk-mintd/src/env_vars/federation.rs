use std::env;
use std::path::PathBuf;

use crate::config::Federation;

pub const ENV_FEDERATION_ENABLED: &str = "CDK_MINTD_FEDERATION_ENABLED";
pub const ENV_FEDERATION_PUBLIC_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH";
pub const ENV_FEDERATION_PRIVATE_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH";

impl Federation {
    pub fn from_env(mut self) -> Self {
        if let Ok(enabled_str) = env::var(ENV_FEDERATION_ENABLED) {
            self.enabled = enabled_str.parse::<bool>().unwrap_or(false);
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PUBLIC_CONFIG_PATH) {
            self.public_config_path = Some(PathBuf::from(path));
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PRIVATE_CONFIG_PATH) {
            self.private_config_path = Some(PathBuf::from(path));
        }

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        ENV_LOCK
            .lock()
            .expect("federation env test lock should not be poisoned")
    }

    #[test]
    fn federation_env_overrides_paths_and_enabled() {
        let _guard = env_lock();
        env::set_var(ENV_FEDERATION_ENABLED, "true");
        env::set_var(ENV_FEDERATION_PUBLIC_CONFIG_PATH, "/tmp/public-fed.json");
        env::set_var(ENV_FEDERATION_PRIVATE_CONFIG_PATH, "/tmp/private-fed.json");

        let config = Federation::default().from_env();

        assert!(config.enabled);
        assert_eq!(
            config.public_config_path,
            Some(PathBuf::from("/tmp/public-fed.json"))
        );
        assert_eq!(
            config.private_config_path,
            Some(PathBuf::from("/tmp/private-fed.json"))
        );

        env::remove_var(ENV_FEDERATION_ENABLED);
        env::remove_var(ENV_FEDERATION_PUBLIC_CONFIG_PATH);
        env::remove_var(ENV_FEDERATION_PRIVATE_CONFIG_PATH);
    }
}
