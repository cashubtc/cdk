use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

use crate::config::{Federation, FederationConsensusBackend};

pub const ENV_FEDERATION_ENABLED: &str = "CDK_MINTD_FEDERATION_ENABLED";
pub const ENV_FEDERATION_PUBLIC_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH";
pub const ENV_FEDERATION_PRIVATE_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH";
pub const ENV_FEDERATION_CONSENSUS_BACKEND: &str = "CDK_MINTD_FEDERATION_CONSENSUS_BACKEND";
pub const ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND: &str =
    "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND";

impl Federation {
    pub fn from_env(mut self) -> Result<Self> {
        if let Ok(enabled_str) = env::var(ENV_FEDERATION_ENABLED) {
            self.enabled = enabled_str.parse::<bool>().unwrap_or(false);
        }

        if let Ok(backend) = env::var(ENV_FEDERATION_CONSENSUS_BACKEND) {
            self.consensus_backend = backend
                .parse::<FederationConsensusBackend>()
                .map_err(|err| anyhow!(err))?;
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PUBLIC_CONFIG_PATH) {
            self.public_config_path = Some(PathBuf::from(path));
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PRIVATE_CONFIG_PATH) {
            self.private_config_path = Some(PathBuf::from(path));
        }

        if let Ok(max_round) = env::var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND) {
            let max_round = max_round.parse::<u16>().map_err(|_| {
                anyhow!("Federation AlephBFT live session max round must fit in u16")
            })?;
            if max_round == 0 {
                return Err(anyhow!(
                    "Federation AlephBFT live session max round must be greater than zero"
                ));
            }
            self.peer_sync.aleph_bft_live_session_max_round = Some(max_round);
        }

        Ok(self)
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

    fn clear_federation_env() {
        env::remove_var(ENV_FEDERATION_ENABLED);
        env::remove_var(ENV_FEDERATION_PUBLIC_CONFIG_PATH);
        env::remove_var(ENV_FEDERATION_PRIVATE_CONFIG_PATH);
        env::remove_var(ENV_FEDERATION_CONSENSUS_BACKEND);
        env::remove_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND);
    }

    #[test]
    fn federation_env_overrides_paths_and_enabled() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ENABLED, "true");
        env::set_var(ENV_FEDERATION_PUBLIC_CONFIG_PATH, "/tmp/public-fed.json");
        env::set_var(ENV_FEDERATION_PRIVATE_CONFIG_PATH, "/tmp/private-fed.json");

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert!(config.enabled);
        assert_eq!(
            config.public_config_path,
            Some(PathBuf::from("/tmp/public-fed.json"))
        );
        assert_eq!(
            config.private_config_path,
            Some(PathBuf::from("/tmp/private-fed.json"))
        );
        assert_eq!(
            config.consensus_backend,
            FederationConsensusBackend::InMemory
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_overrides_consensus_backend() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_CONSENSUS_BACKEND, "aleph_bft");

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert_eq!(
            config.consensus_backend,
            FederationConsensusBackend::AlephBft
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_rejects_unknown_consensus_backend() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_CONSENSUS_BACKEND, "raft");

        let err = Federation::default()
            .from_env()
            .expect_err("unknown consensus backend should fail");

        assert!(
            err.to_string()
                .contains("Unknown federation consensus backend"),
            "error should mention unknown backend: {err}"
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_overrides_aleph_bft_live_session_max_round() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND, "9");

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert_eq!(config.peer_sync.aleph_bft_live_session_max_round, Some(9));

        clear_federation_env();
    }

    #[test]
    fn federation_env_rejects_invalid_aleph_bft_live_session_max_round() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND, "0");

        let err = Federation::default()
            .from_env()
            .expect_err("zero max round should fail");

        assert!(
            err.to_string().contains("must be greater than zero"),
            "error should explain invalid max round: {err}"
        );

        clear_federation_env();
    }
}
