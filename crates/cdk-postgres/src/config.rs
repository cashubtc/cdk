use std::fmt;
use std::time::Duration;

use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;

mod connection_string;

/// PostgreSQL connection and pool settings.
#[derive(Clone)]
pub struct PgConfig {
    pub(crate) url: String,
    pub(crate) schema: Option<String>,
    tls_mode: Option<String>,
    inferred_tls_mode: Option<String>,
    invalid_connection_string: bool,
    pub(crate) max_connections: usize,
    pub(crate) connection_timeout: Duration,
}

impl fmt::Debug for PgConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgConfig")
            .field("url", &"[redacted]")
            .field("schema", &self.schema)
            .field("tls_mode", &self.tls_mode)
            .field("max_connections", &self.max_connections)
            .field("connection_timeout", &self.connection_timeout)
            .finish()
    }
}

/// Invalid PostgreSQL configuration.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("PostgreSQL max_connections must be greater than zero")]
    EmptyPool,
    #[error("Unsupported PostgreSQL TLS mode")]
    TlsMode,
    #[error("PostgreSQL schema must not be empty or contain a NUL byte")]
    Schema,
    #[error("Invalid PostgreSQL connection string")]
    ConnectionString,
}

// Retain the driver cause for programmatic inspection without printing a
// potentially malformed connection string (or password) in diagnostics.
#[derive(thiserror::Error)]
#[error("Invalid PostgreSQL connection string")]
struct ConnectionStringError(#[source] tokio_postgres::Error);

impl fmt::Debug for ConnectionStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionStringError([redacted])")
    }
}

impl PgConfig {
    /// Create settings, preserving the existing constructor defaults.
    ///
    /// The timeout is applied separately to pool waiting, connection creation,
    /// recycling, and transaction cleanup. Validation occurs when opening the database.
    pub fn new(
        conn_str: &str,
        tls_mode: Option<&str>,
        max_connections: Option<usize>,
        connection_timeout_secs: Option<u64>,
    ) -> Self {
        let parsed = connection_string::parse(conn_str);
        let invalid_connection_string = parsed.is_err();
        let (url, schema, inferred_tls_mode) = parsed.unwrap_or_default();
        Self {
            url,
            schema,
            inferred_tls_mode,
            invalid_connection_string,
            tls_mode: tls_mode.map(str::to_lowercase),
            max_connections: max_connections.unwrap_or(20),
            connection_timeout: Duration::from_secs(connection_timeout_secs.unwrap_or(10)),
        }
    }

    pub(crate) fn driver_config(
        &self,
    ) -> Result<(tokio_postgres::Config, Option<MakeTlsConnector>), cdk_common::database::Error>
    {
        use super::backend::database_error;
        if self.max_connections == 0 {
            return Err(database_error(ConfigError::EmptyPool));
        }
        if self
            .schema
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.contains('\0'))
        {
            return Err(database_error(ConfigError::Schema));
        }
        if self.invalid_connection_string {
            return Err(database_error(ConfigError::ConnectionString));
        }
        let mode = self
            .tls_mode
            .as_deref()
            .or(self.inferred_tls_mode.as_deref())
            .unwrap_or("disable");
        let mut driver: tokio_postgres::Config = self
            .url
            .parse()
            .map_err(|error| database_error(ConnectionStringError(error)))?;
        let (ssl, invalid_certs, invalid_hostnames) = match mode {
            "disable" => {
                driver.ssl_mode(tokio_postgres::config::SslMode::Disable);
                return Ok((driver, None));
            }
            "prefer" | "allow" => (tokio_postgres::config::SslMode::Prefer, true, true),
            "require" => (tokio_postgres::config::SslMode::Require, true, true),
            "verify-ca" => (tokio_postgres::config::SslMode::Require, false, true),
            "verify-full" => (tokio_postgres::config::SslMode::Require, false, false),
            _ => return Err(database_error(ConfigError::TlsMode)),
        };
        driver.ssl_mode(ssl);
        let tls = TlsConnector::builder()
            .danger_accept_invalid_certs(invalid_certs)
            .danger_accept_invalid_hostnames(invalid_hostnames)
            .build()
            .map_err(database_error)?;
        Ok((driver, Some(MakeTlsConnector::new(tls))))
    }
}

impl From<&str> for PgConfig {
    fn from(value: &str) -> Self {
        Self::new(value, None, None, None)
    }
}

#[cfg(test)]
mod tests;
