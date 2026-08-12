//! Database environment variables

use std::env;
use std::str::FromStr;

use crate::config::{PostgresAuthConfig, PostgresConfig, PubSubConfig, PubSubTransport};

pub const ENV_POSTGRES_URL: &str = "CDK_MINTD_POSTGRES_URL";
pub const ENV_POSTGRES_TLS_MODE: &str = "CDK_MINTD_POSTGRES_TLS_MODE";
pub const ENV_POSTGRES_MAX_CONNECTIONS: &str = "CDK_MINTD_POSTGRES_MAX_CONNECTIONS";
pub const ENV_POSTGRES_CONNECTION_TIMEOUT: &str = "CDK_MINTD_POSTGRES_CONNECTION_TIMEOUT_SECONDS";

pub const ENV_PUBSUB_TRANSPORT: &str = "CDK_MINTD_PUBSUB_TRANSPORT";
pub const ENV_PUBSUB_CHANNEL: &str = "CDK_MINTD_PUBSUB_CHANNEL";
pub const ENV_PUBSUB_POLL_INTERVAL_MS: &str = "CDK_MINTD_PUBSUB_POLL_INTERVAL_MS";
pub const ENV_PUBSUB_RETENTION_SECONDS: &str = "CDK_MINTD_PUBSUB_RETENTION_SECONDS";

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
    pub fn from_env(mut self) -> Self {
        if let Ok(transport) = env::var(ENV_PUBSUB_TRANSPORT) {
            if let Ok(parsed) = PubSubTransport::from_str(&transport) {
                self.transport = parsed;
            }
        }

        if let Ok(channel) = env::var(ENV_PUBSUB_CHANNEL) {
            self.channel = Some(channel);
        }

        if let Ok(interval) = env::var(ENV_PUBSUB_POLL_INTERVAL_MS) {
            if let Ok(parsed) = interval.parse::<u64>() {
                self.poll_interval_ms = Some(parsed);
            }
        }

        if let Ok(retention) = env::var(ENV_PUBSUB_RETENTION_SECONDS) {
            if let Ok(parsed) = retention.parse::<u64>() {
                self.retention_seconds = Some(parsed);
            }
        }

        self
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
