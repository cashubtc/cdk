//! PostgreSQL connection settings.

use std::fmt;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::pool::DatabaseConfig;

mod connection_string;

use crate::tls::{self, TlsPolicy};
use crate::SslMode;

/// Default maximum number of connections in the pool
const DEFAULT_MAX_CONNECTIONS: usize = 20;

/// Default connection timeout in seconds
const DEFAULT_CONNECTION_TIMEOUT_SECS: u64 = 10;

/// Postgres configuration
#[derive(Clone)]
pub struct PgConfig {
    url: String,
    pub(crate) schema: Option<String>,
    pub(crate) legacy_schema: Option<String>,
    tls_mode: Option<String>,
    inferred_tls_policy: Option<TlsPolicy>,
    parse_error: Option<ConfigError>,
    max_connections: usize,
    connection_timeout: Duration,
}

impl fmt::Debug for PgConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgConfig")
            .field("url", &"[redacted]")
            .field("schema", &self.schema)
            .field("tls_mode", &self.tls_mode.as_ref().map(|_| "[configured]"))
            .field("max_connections", &self.max_connections)
            .field("connection_timeout", &self.connection_timeout)
            .finish()
    }
}

impl DatabaseConfig for PgConfig {
    fn default_timeout(&self) -> Duration {
        self.connection_timeout
    }

    fn max_size(&self) -> usize {
        self.max_connections
    }
}

/// Invalid PostgreSQL configuration.
///
/// Messages never include connection-string contents, which may hold a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("Invalid PostgreSQL TLS mode")]
    TlsMode,
    #[error("PostgreSQL schema must not be empty or contain a NUL byte")]
    Schema,
    #[error("Invalid PostgreSQL connection string")]
    ConnectionString,
    #[error(
        "The configured PostgreSQL schema name contains quotes or escapes, and a schema \
         with the literal name earlier versions used exists. Rename that schema to the \
         configured name, or drop it if unused, before starting"
    )]
    LegacySchema,
}

impl From<ConfigError> for Error {
    fn from(error: ConfigError) -> Self {
        Self::Database(Box::new(error))
    }
}

impl PgConfig {
    /// Create a new `PgConfig` with explicit TLS mode, pool size, and timeout.
    ///
    /// `tls_mode` accepts the same strings as the configuration file:
    /// `"disable"`, `"prefer"`, `"allow"`, `"require"`, `"verify-ca"`,
    /// `"verify-full"`.  When `None`, the TLS mode is inferred from
    /// `sslmode=` in the connection URL. With neither setting, TLS is disabled.
    /// `allow` uses the same opportunistic TLS policy as `prefer`.
    ///
    /// The connection string may also carry a `schema` parameter: a
    /// `schema=` keyword (quoted and escaped like any other keyword value,
    /// and also accepted after a URI), or a percent-encoded `schema` URI query
    /// parameter. Invalid settings and TLS connector errors are returned when
    /// validating or connecting.
    pub fn new(
        conn_str: &str,
        tls_mode: Option<&str>,
        max_connections: Option<usize>,
        connection_timeout_secs: Option<u64>,
    ) -> Self {
        let (parsed, parse_error) = match connection_string::parse(conn_str) {
            Ok(parsed) => (parsed, None),
            Err(error) => (Default::default(), Some(error)),
        };
        Self {
            url: parsed.url,
            schema: parsed.schema,
            legacy_schema: parsed.legacy_schema,
            tls_mode: tls_mode.map(str::to_owned),
            inferred_tls_policy: parsed.tls_policy,
            parse_error,
            max_connections: max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS),
            connection_timeout: Duration::from_secs(
                connection_timeout_secs.unwrap_or(DEFAULT_CONNECTION_TIMEOUT_SECS),
            ),
        }
    }

    /// Validate connection parameters and construct the configured TLS connector.
    ///
    /// Does not open a connection or verify the server's certificate. Uses the
    /// same TLS policy and connector construction as connection establishment.
    pub fn validate(&self) -> Result<(), Error> {
        self.driver_config().map(|_| ())
    }

    /// Compare effective TLS policies, including certificate verification requirements.
    ///
    /// Resolves explicit modes, URL modes, and defaults using the connection policy.
    /// Invalid settings return an error. Does not connect or construct TLS connectors.
    pub fn has_same_tls_policy(&self, other: &Self) -> Result<bool, Error> {
        let (_, policy) = self.resolve_driver()?;
        let (_, other_policy) = other.resolve_driver()?;
        Ok(policy == other_policy)
    }

    fn resolve_driver(&self) -> Result<(tokio_postgres::Config, TlsPolicy), Error> {
        if let Some(error) = self.parse_error {
            return Err(error.into());
        }
        let policy = match self.tls_mode.as_deref() {
            Some(mode) => TlsPolicy::parse(mode)?,
            None => self.inferred_tls_policy.unwrap_or(TlsPolicy::Disable),
        };
        if self
            .schema
            .as_ref()
            .is_some_and(|schema| schema.is_empty() || schema.contains('\0'))
        {
            return Err(ConfigError::Schema.into());
        }
        // Discard the driver error so malformed credentials cannot leak through
        // error source chains (for example `UnknownOption` printed by `anyhow`).
        let driver: tokio_postgres::Config = self
            .url
            .parse()
            .map_err(|_| ConfigError::ConnectionString)?;
        Ok((driver, policy))
    }

    /// Build the driver configuration and TLS connector used to connect.
    pub(crate) fn driver_config(&self) -> Result<(tokio_postgres::Config, SslMode), Error> {
        let (mut driver, policy) = self.resolve_driver()?;
        let tls = tls::configure(&mut driver, policy)?;
        Ok((driver, tls))
    }
}

impl From<&str> for PgConfig {
    fn from(conn_str: &str) -> Self {
        Self::new(conn_str, None, None, None)
    }
}

#[cfg(test)]
mod tests;
