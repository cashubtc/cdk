use std::env;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

use crate::config::Federation;

pub const ENV_FEDERATION_ENABLED: &str = "CDK_MINTD_FEDERATION_ENABLED";
pub const ENV_FEDERATION_PUBLIC_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH";
pub const ENV_FEDERATION_PRIVATE_CONFIG_PATH: &str = "CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH";
pub const ENV_FEDERATION_RESTORE_MANIFEST_PATH: &str = "CDK_MINTD_FEDERATION_RESTORE_MANIFEST_PATH";
pub const ENV_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS: &str =
    "CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS";
pub const ENV_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS: &str =
    "CDK_MINTD_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS";
pub const ENV_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK: &str =
    "CDK_MINTD_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK";
pub const ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND: &str =
    "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND";
pub const ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT: &str =
    "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT";
pub const ENV_FEDERATION_ONCHAIN_WALLET_ENABLED: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_ENABLED";
pub const ENV_FEDERATION_ONCHAIN_WALLET_NETWORK: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_NETWORK";
pub const ENV_FEDERATION_ONCHAIN_WALLET_MINIMUM_CONFIRMATIONS: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_MINIMUM_CONFIRMATIONS";
pub const ENV_FEDERATION_ONCHAIN_WALLET_FEE_POLICY_VERSION: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_FEE_POLICY_VERSION";
pub const ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_TYPE: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_TYPE";
pub const ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_SCOPE: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_SCOPE";
pub const ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_ID: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_ID";
pub const ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_URL: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_ESPLORA_URL";
pub const ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_PARALLEL_REQUESTS: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_ESPLORA_PARALLEL_REQUESTS";
pub const ENV_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS";
pub const ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_MAX_RETRIES: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_ESPLORA_MAX_RETRIES";
pub const ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_HOST: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_HOST";
pub const ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PORT: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PORT";
pub const ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_USER: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_USER";
pub const ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PASSWORD: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PASSWORD";
pub const ENV_FEDERATION_ONCHAIN_WALLET_SYNC_INTERVAL_SECS: &str =
    "CDK_MINTD_FEDERATION_ONCHAIN_WALLET_SYNC_INTERVAL_SECS";

impl Federation {
    pub fn from_env(mut self) -> Result<Self> {
        if let Ok(enabled_str) = env::var(ENV_FEDERATION_ENABLED) {
            self.enabled = enabled_str.parse::<bool>().unwrap_or(false);
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PUBLIC_CONFIG_PATH) {
            self.public_config_path = Some(PathBuf::from(path));
        }

        if let Ok(path) = env::var(ENV_FEDERATION_PRIVATE_CONFIG_PATH) {
            self.private_config_path = Some(PathBuf::from(path));
        }

        if let Ok(path) = env::var(ENV_FEDERATION_RESTORE_MANIFEST_PATH) {
            self.restore_manifest_path = Some(PathBuf::from(path));
        }

        if let Ok(timeout) = env::var(ENV_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS) {
            let timeout = timeout.parse::<u64>().map_err(|_| {
                anyhow!("Federation operation submission timeout must fit in u64 seconds")
            })?;
            if timeout == 0 {
                return Err(anyhow!(
                    "Federation operation submission timeout must be greater than zero"
                ));
            }
            self.peer_sync.operation_submission_timeout_secs = timeout;
        }

        if let Ok(interval) = env::var(ENV_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS) {
            let interval = interval.parse::<u64>().map_err(|_| {
                anyhow!("Federation AlephBFT active unit interval must fit in u64 milliseconds")
            })?;
            if interval == 0 {
                return Err(anyhow!(
                    "Federation AlephBFT active unit interval must be greater than zero"
                ));
            }
            self.peer_sync.aleph_bft_active_unit_interval_ms = interval;
        }

        if let Ok(limit) = env::var(ENV_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK) {
            let limit = limit
                .parse::<usize>()
                .map_err(|_| anyhow!("Federation AlephBFT network pump limit must fit in usize"))?;
            if limit == 0 {
                return Err(anyhow!(
                    "Federation AlephBFT network pump limit must be greater than zero"
                ));
            }
            self.peer_sync.aleph_bft_network_pump_max_messages_per_tick = limit;
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

        if let Ok(rotation_limit) = env::var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT) {
            let rotation_limit = rotation_limit.parse::<usize>().map_err(|_| {
                anyhow!("Federation AlephBFT live session rotation limit must fit in usize")
            })?;
            if rotation_limit == 0 {
                return Err(anyhow!(
                    "Federation AlephBFT live session rotation limit must be greater than zero"
                ));
            }
            self.peer_sync.aleph_bft_live_session_rotation_limit = Some(rotation_limit);
        }

        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_ENABLED) {
            self.onchain_wallet.enabled = parse_env(&value, "enabled")?;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_NETWORK) {
            self.onchain_wallet.network = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_MINIMUM_CONFIRMATIONS) {
            self.onchain_wallet.minimum_confirmations =
                Some(parse_nonzero_env(&value, "minimum confirmations")?);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_FEE_POLICY_VERSION) {
            self.onchain_wallet.fee_policy_version =
                Some(parse_nonzero_env(&value, "fee policy version")?);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_TYPE) {
            self.onchain_wallet.chain_source_type = value;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_SCOPE) {
            self.onchain_wallet.chain_source_scope = value;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_ID) {
            self.onchain_wallet.chain_source_id = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_URL) {
            self.onchain_wallet.esplora_url = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_PARALLEL_REQUESTS) {
            self.onchain_wallet.esplora_parallel_requests =
                parse_nonzero_env(&value, "Esplora parallel requests")?;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS) {
            self.onchain_wallet.request_timeout_secs =
                parse_nonzero_env(&value, "request timeout")?;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_MAX_RETRIES) {
            self.onchain_wallet.esplora_max_retries = parse_env(&value, "Esplora max retries")?;
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_HOST) {
            self.onchain_wallet.bitcoind_rpc_host = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PORT) {
            self.onchain_wallet.bitcoind_rpc_port =
                Some(parse_nonzero_env(&value, "Bitcoin RPC port")?);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_USER) {
            self.onchain_wallet.bitcoind_rpc_user = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PASSWORD) {
            self.onchain_wallet.bitcoind_rpc_password = Some(value);
        }
        if let Ok(value) = env::var(ENV_FEDERATION_ONCHAIN_WALLET_SYNC_INTERVAL_SECS) {
            self.onchain_wallet.sync_interval_secs = parse_nonzero_env(&value, "sync interval")?;
        }

        Ok(self)
    }
}

fn parse_env<T>(value: &str, field: &str) -> Result<T>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| anyhow!("Federation on-chain wallet {field} is invalid"))
}

fn parse_nonzero_env<T>(value: &str, field: &str) -> Result<T>
where
    T: std::str::FromStr + Default + PartialEq,
{
    let parsed = parse_env(value, field)?;
    if parsed == T::default() {
        return Err(anyhow!(
            "Federation on-chain wallet {field} must be greater than zero"
        ));
    }
    Ok(parsed)
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
        env::remove_var(ENV_FEDERATION_RESTORE_MANIFEST_PATH);
        env::remove_var(ENV_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS);
        env::remove_var(ENV_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS);
        env::remove_var(ENV_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK);
        env::remove_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND);
        env::remove_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_ENABLED);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_NETWORK);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_MINIMUM_CONFIRMATIONS);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_FEE_POLICY_VERSION);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_TYPE);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_SCOPE);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_ID);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_URL);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_PARALLEL_REQUESTS);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_MAX_RETRIES);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_HOST);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PORT);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_USER);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_BITCOIND_RPC_PASSWORD);
        env::remove_var(ENV_FEDERATION_ONCHAIN_WALLET_SYNC_INTERVAL_SECS);
    }

    #[test]
    fn federation_env_overrides_paths_and_enabled() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ENABLED, "true");
        env::set_var(ENV_FEDERATION_PUBLIC_CONFIG_PATH, "/tmp/public-fed.json");
        env::set_var(ENV_FEDERATION_PRIVATE_CONFIG_PATH, "/tmp/private-fed.json");
        env::set_var(
            ENV_FEDERATION_RESTORE_MANIFEST_PATH,
            "/tmp/restore-manifest.json",
        );

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
            config.restore_manifest_path,
            Some(PathBuf::from("/tmp/restore-manifest.json"))
        );
        clear_federation_env();
    }

    #[test]
    fn federation_env_overrides_aleph_bft_active_unit_and_pump_settings() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS, "100");
        env::set_var(
            ENV_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK,
            "8",
        );

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert_eq!(config.peer_sync.aleph_bft_active_unit_interval_ms, 100);
        assert_eq!(
            config
                .peer_sync
                .aleph_bft_network_pump_max_messages_per_tick,
            8
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_rejects_zero_aleph_bft_active_unit_and_pump_settings() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS, "0");
        let interval_err = Federation::default()
            .from_env()
            .expect_err("zero active unit interval should fail");
        assert!(
            interval_err
                .to_string()
                .contains("must be greater than zero"),
            "error should explain invalid active unit interval: {interval_err}"
        );

        clear_federation_env();
        env::set_var(
            ENV_FEDERATION_ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK,
            "0",
        );
        let pump_err = Federation::default()
            .from_env()
            .expect_err("zero pump limit should fail");
        assert!(
            pump_err.to_string().contains("must be greater than zero"),
            "error should explain invalid pump limit: {pump_err}"
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
    fn federation_env_overrides_aleph_bft_live_session_rotation_limit() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT, "3");

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert_eq!(
            config.peer_sync.aleph_bft_live_session_rotation_limit,
            Some(3)
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_overrides_operation_submission_timeout() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS, "42");

        let config = Federation::default()
            .from_env()
            .expect("federation env config");

        assert_eq!(config.peer_sync.operation_submission_timeout_secs, 42);

        clear_federation_env();
    }

    #[test]
    fn federation_env_rejects_invalid_operation_submission_timeout() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS, "0");

        let err = Federation::default()
            .from_env()
            .expect_err("zero operation timeout should fail");

        assert!(
            err.to_string().contains("must be greater than zero"),
            "error should explain invalid operation timeout: {err}"
        );

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

    #[test]
    fn federation_env_rejects_invalid_aleph_bft_live_session_rotation_limit() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT, "0");

        let err = Federation::default()
            .from_env()
            .expect_err("zero rotation limit should fail");

        assert!(
            err.to_string().contains("must be greater than zero"),
            "error should explain invalid rotation limit: {err}"
        );

        clear_federation_env();
    }

    #[test]
    fn federation_env_overrides_complete_onchain_wallet_config() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_ENABLED, "true");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_NETWORK, "regtest");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_MINIMUM_CONFIRMATIONS, "2");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_FEE_POLICY_VERSION, "1");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_TYPE, "esplora");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_SCOPE, "shared");
        env::set_var(
            ENV_FEDERATION_ONCHAIN_WALLET_CHAIN_SOURCE_ID,
            "shared-regtest",
        );
        env::set_var(
            ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_URL,
            "http://127.0.0.1:3002",
        );
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_PARALLEL_REQUESTS, "3");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS, "20");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_ESPLORA_MAX_RETRIES, "4");
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_SYNC_INTERVAL_SECS, "11");

        let config = Federation::default()
            .from_env()
            .expect("federation wallet env config");
        let wallet = config.onchain_wallet;
        assert!(wallet.enabled);
        assert_eq!(wallet.network.as_deref(), Some("regtest"));
        assert_eq!(wallet.minimum_confirmations, Some(2));
        assert_eq!(wallet.fee_policy_version, Some(1));
        assert_eq!(wallet.chain_source_type, "esplora");
        assert_eq!(wallet.chain_source_scope, "shared");
        assert_eq!(wallet.chain_source_id.as_deref(), Some("shared-regtest"));
        assert_eq!(wallet.esplora_parallel_requests, 3);
        assert_eq!(wallet.request_timeout_secs, 20);
        assert_eq!(wallet.esplora_max_retries, 4);
        assert_eq!(wallet.sync_interval_secs, 11);
        clear_federation_env();
    }

    #[test]
    fn federation_env_rejects_zero_wallet_timeout() {
        let _guard = env_lock();
        clear_federation_env();
        env::set_var(ENV_FEDERATION_ONCHAIN_WALLET_REQUEST_TIMEOUT_SECS, "0");

        let error = Federation::default()
            .from_env()
            .expect_err("zero wallet request timeout must fail");
        assert!(error.to_string().contains("must be greater than zero"));
        clear_federation_env();
    }
}
