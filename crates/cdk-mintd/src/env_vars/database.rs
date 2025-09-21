//! Database environment variables

use std::env;

use crate::config::{PostgresAuthConfig, PostgresConfig};

pub const ENV_POSTGRES_URL: &str = "CDK_MINTD_POSTGRES_URL";
pub const ENV_POSTGRES_TLS_MODE: &str = "CDK_MINTD_POSTGRES_TLS_MODE";
pub const ENV_POSTGRES_MAX_CONNECTIONS: &str = "CDK_MINTD_POSTGRES_MAX_CONNECTIONS";
pub const ENV_POSTGRES_CONNECTION_TIMEOUT: &str = "CDK_MINTD_POSTGRES_CONNECTION_TIMEOUT_SECONDS";

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
