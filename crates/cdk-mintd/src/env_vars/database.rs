//! Database environment variables

use std::env;

use anyhow::{anyhow, bail, Result};

use crate::config::{PostgresAuthConfig, PostgresConfig, PubSubConfig};

pub const ENV_POSTGRES_URL: &str = "CDK_MINTD_POSTGRES_URL";
pub const ENV_POSTGRES_TLS_MODE: &str = "CDK_MINTD_POSTGRES_TLS_MODE";
pub const ENV_POSTGRES_MAX_CONNECTIONS: &str = "CDK_MINTD_POSTGRES_MAX_CONNECTIONS";
pub const ENV_POSTGRES_CONNECTION_TIMEOUT: &str = "CDK_MINTD_POSTGRES_CONNECTION_TIMEOUT_SECONDS";

pub const ENV_PUBSUB_CROSS_INSTANCE: &str = "CDK_MINTD_PUBSUB_CROSS_INSTANCE";
/// Removed, kept to fail a deployment that still sets it.
pub const ENV_PUBSUB_TRANSPORT: &str = "CDK_MINTD_PUBSUB_TRANSPORT";
/// Removed, kept to fail a deployment that still sets it.
pub const ENV_PUBSUB_CHANNEL: &str = "CDK_MINTD_PUBSUB_CHANNEL";

pub const ENV_AUTH_POSTGRES_URL: &str = "CDK_MINTD_AUTH_POSTGRES_URL";
pub const ENV_AUTH_POSTGRES_TLS_MODE: &str = "CDK_MINTD_AUTH_POSTGRES_TLS_MODE";
pub const ENV_AUTH_POSTGRES_MAX_CONNECTIONS: &str = "CDK_MINTD_AUTH_POSTGRES_MAX_CONNECTIONS";
pub const ENV_AUTH_POSTGRES_CONNECTION_TIMEOUT: &str =
    "CDK_MINTD_AUTH_POSTGRES_CONNECTION_TIMEOUT_SECONDS";

impl PostgresConfig {
    pub fn from_env(mut self) -> Self {
        // Check for new PostgreSQL URL env var first, then fallback to legacy DATABASE_URL
        if let Ok(url) = env::var(ENV_POSTGRES_URL) {
            self.url = url;
        } else if let Ok(url) = env::var(super::DATABASE_URL_ENV_VAR) {
            // Backward compatibility with the existing DATABASE_URL env var
            self.url = url;
        }

        if let Ok(tls_mode) = env::var(ENV_POSTGRES_TLS_MODE) {
            self.tls_mode = Some(tls_mode);
        }

        if let Ok(max_connections) = env::var(ENV_POSTGRES_MAX_CONNECTIONS) {
            if let Ok(parsed) = max_connections.parse::<usize>() {
                self.max_connections = Some(parsed);
            }
        }

        if let Ok(timeout) = env::var(ENV_POSTGRES_CONNECTION_TIMEOUT) {
            if let Ok(parsed) = timeout.parse::<u64>() {
                self.connection_timeout_seconds = Some(parsed);
            }
        }

        self
    }
}

impl PubSubConfig {
    /// Returns an error on an unusable value, and on either removed variable,
    /// rather than falling back to the default, so an operator carrying an old
    /// setting learns at startup instead of silently getting the other
    /// behaviour.
    pub fn from_env(mut self) -> Result<Self> {
        for removed in [ENV_PUBSUB_TRANSPORT, ENV_PUBSUB_CHANNEL] {
            if env::var(removed).is_ok() {
                bail!(
                    "{removed} was removed: cross-instance notifications now follow the database \
                     engine, on a channel internal to the mint. Set \
                     {ENV_PUBSUB_CROSS_INSTANCE}=false to keep notifications in-process"
                );
            }
        }

        if let Ok(cross_instance) = env::var(ENV_PUBSUB_CROSS_INSTANCE) {
            self.cross_instance = cross_instance
                .parse()
                .map_err(|err| anyhow!("{ENV_PUBSUB_CROSS_INSTANCE}: {err}"))?;
        }

        Ok(self)
    }
}

impl PostgresAuthConfig {
    pub fn from_env(mut self) -> Self {
        if let Ok(url) = env::var(ENV_AUTH_POSTGRES_URL) {
            self.url = url;
        }

        if let Ok(tls_mode) = env::var(ENV_AUTH_POSTGRES_TLS_MODE) {
            self.tls_mode = Some(tls_mode);
        }

        if let Ok(max_connections) = env::var(ENV_AUTH_POSTGRES_MAX_CONNECTIONS) {
            if let Ok(parsed) = max_connections.parse::<usize>() {
                self.max_connections = Some(parsed);
            }
        }

        if let Ok(timeout) = env::var(ENV_AUTH_POSTGRES_CONNECTION_TIMEOUT) {
            if let Ok(parsed) = timeout.parse::<u64>() {
                self.connection_timeout_seconds = Some(parsed);
            }
        }

        self
    }
}
