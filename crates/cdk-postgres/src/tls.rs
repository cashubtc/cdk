//! Resolve connector verification and PostgreSQL negotiation from one TLS policy.

use cdk_common::database::Error;
use native_tls::{TlsConnector, TlsConnectorBuilder};
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::config::SslMode as PgSslMode;
use tokio_postgres::{Config, NoTls};

use crate::SslMode;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TlsPolicy {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl TlsPolicy {
    fn parse(value: &str) -> Result<Self, Error> {
        match value.to_ascii_lowercase().as_str() {
            "disable" => Ok(Self::Disable),
            "allow" | "prefer" => Ok(Self::Prefer),
            "require" => Ok(Self::Require),
            "verify-ca" => Ok(Self::VerifyCa),
            "verify-full" => Ok(Self::VerifyFull),
            // Do not echo arbitrary connection-string contents in errors.
            _ => Err(Error::Internal("Invalid PostgreSQL TLS mode".to_owned())),
        }
    }

    fn negotiation(self) -> PgSslMode {
        match self {
            Self::Disable => PgSslMode::Disable,
            Self::Prefer => PgSslMode::Prefer,
            Self::Require | Self::VerifyCa | Self::VerifyFull => PgSslMode::Require,
        }
    }

    fn build_connector<F>(self, build: F) -> Result<SslMode, Error>
    where
        F: FnOnce(TlsConnectorBuilder) -> Result<TlsConnector, native_tls::Error>,
    {
        if matches!(self, Self::Disable) {
            return Ok(SslMode::NoTls(NoTls));
        }

        let mut builder = TlsConnector::builder();
        builder.danger_accept_invalid_certs(matches!(self, Self::Prefer | Self::Require));
        builder.danger_accept_invalid_hostnames(!matches!(self, Self::VerifyFull));
        let connector = build(builder).map_err(|err| Error::Database(Box::new(err)))?;
        Ok(SslMode::NativeTls(MakeTlsConnector::new(connector)))
    }
}

pub(super) fn configure(input: &str, explicit: Option<&str>) -> Result<(Config, SslMode), Error> {
    let (config, policy) = resolve(input, explicit)?;
    Ok((config, policy.build_connector(|builder| builder.build())?))
}

pub(super) fn resolve(input: &str, explicit: Option<&str>) -> Result<(Config, TlsPolicy), Error> {
    let (normalized, url_policy) = normalize(input)?;
    let policy = match explicit {
        Some(mode) => TlsPolicy::parse(mode)?,
        None => url_policy.unwrap_or(TlsPolicy::Disable),
    };
    let mut config: Config = normalized
        .parse()
        .map_err(|err| Error::Database(Box::new(err)))?;
    config.ssl_mode(policy.negotiation());
    Ok((config, policy))
}

// tokio-postgres only parses disable/prefer/require. Extract the complete mode
// and replace it with a supported placeholder before invoking its parser. All
// modes, including overridden values, are validated to catch configuration typos.
fn normalize(input: &str) -> Result<(String, Option<TlsPolicy>), Error> {
    let mut policy = None;
    let mut output = String::new();
    if input.starts_with("postgres://") || input.starts_with("postgresql://") {
        let Some((base, query)) = input.split_once('?') else {
            return Ok((input.to_owned(), None));
        };
        if query.is_empty() {
            return Ok((input.to_owned(), None));
        }
        output.push_str(base);
        output.push('?');
        for (index, parameter) in query.split('&').enumerate() {
            if index != 0 {
                output.push('&');
            }
            let (key, value) = parameter.split_once('=').ok_or_else(invalid_parameters)?;
            let key = percent_encoding::percent_decode_str(key)
                .decode_utf8()
                .map_err(|_| invalid_parameters())?;
            if key == "sslmode" {
                let value = percent_encoding::percent_decode_str(value)
                    .decode_utf8()
                    .map_err(|_| invalid_parameters())?;
                policy = Some(TlsPolicy::parse(&value)?);
                output.push_str("sslmode=disable");
            } else {
                output.push_str(parameter);
            }
        }
    } else {
        // Respect libpq keyword syntax: whitespace around '=', quoted values,
        // and backslash escapes. Never search inside passwords or other values.
        let mut rest = input;
        while !rest.trim_start().is_empty() {
            rest = rest.trim_start();
            let start = rest;
            let key_end = rest
                .find(|ch: char| ch == '=' || ch.is_whitespace())
                .ok_or_else(invalid_parameters)?;
            let key = &rest[..key_end];
            rest = rest[key_end..].trim_start();
            rest = rest
                .strip_prefix('=')
                .ok_or_else(invalid_parameters)?
                .trim_start();
            let quoted = rest.starts_with('\'');
            if quoted {
                rest = &rest[1..];
            }
            let mut value = String::new();
            let mut chars = rest.char_indices();
            let mut end = rest.len();
            let mut closed = !quoted;
            while let Some((index, ch)) = chars.next() {
                match ch {
                    '\\' => {
                        let (_, escaped) = chars.next().ok_or_else(invalid_parameters)?;
                        value.push(escaped);
                    }
                    '\'' if quoted => {
                        end = index + 1;
                        closed = true;
                        break;
                    }
                    ch if !quoted && ch.is_whitespace() => {
                        end = index;
                        break;
                    }
                    ch => value.push(ch),
                }
            }
            if !closed || (!quoted && value.is_empty()) {
                return Err(invalid_parameters());
            }
            rest = &rest[end..];
            if key == "sslmode" {
                policy = Some(TlsPolicy::parse(&value)?);
                output.push_str("sslmode=disable");
            } else {
                output.push_str(&start[..start.len() - rest.len()]);
            }
            output.push(' ');
        }
    }
    Ok((output, policy))
}

fn invalid_parameters() -> Error {
    Error::Internal("Invalid PostgreSQL connection parameters".to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;
    use crate::{PgConfig, PostgresConnection};

    #[test]
    fn tls_connector_construction_errors_never_disable_tls() {
        // Inject a native TLS error because platform connector-construction
        // failures cannot be triggered portably through builder settings.
        let error = native_tls::Certificate::from_pem(b"not a certificate")
            .err()
            .expect("invalid certificate");
        let result = TlsPolicy::Require.build_connector(|_| Err(error));
        assert!(matches!(result, Err(Error::Database(_))));
    }

    #[test]
    fn tls_modes_set_connector_and_negotiation_together() {
        for (mode, negotiation) in [
            ("disable", PgSslMode::Disable),
            ("allow", PgSslMode::Prefer),
            ("prefer", PgSslMode::Prefer),
            ("require", PgSslMode::Require),
            ("verify-ca", PgSslMode::Require),
            ("verify-full", PgSslMode::Require),
        ] {
            // An explicit policy overrides the connection-string policy.
            let (config, connector) = configure("host=localhost sslmode=verify-full", Some(mode))
                .expect("explicit TLS mode");
            assert_eq!(config.get_ssl_mode(), negotiation, "{mode}");
            assert_eq!(matches!(connector, SslMode::NoTls(_)), mode == "disable");
        }
        assert_eq!(
            configure("host=localhost", None)
                .expect("default")
                .0
                .get_ssl_mode(),
            PgSslMode::Disable
        );
    }

    #[test]
    fn tls_modes_reject_unknown_and_malformed_settings() {
        let invalid_modes = ["", "requre"]; // typos: ignore
        for mode in invalid_modes {
            assert!(configure("host=localhost", Some(mode)).is_err());
        }
        assert!(configure("host=localhost sslmode='requre'", None).is_err()); // typos: ignore
        assert!(configure("postgres://localhost/db?sslmode=verify-full-extra", None).is_err());
        for input in [
            "host=localhost sslmode='require",
            "host=localhost sslmode=",
            "host=localhost sslmode=invalid sslmode=require",
            "postgres://localhost/db?sslmode=invalid&sslmode=require",
        ] {
            assert!(configure(input, Some("require")).is_err());
        }
    }

    #[test]
    fn tls_modes_parse_parameters_without_matching_credentials() {
        let (config, _) = configure(
            r"host=localhost password='secret sslmode=disable \'quoted\'' sslmode = 'verify-full'",
            None,
        )
        .expect("quoted password");
        assert_eq!(
            config.get_password(),
            Some(b"secret sslmode=disable 'quoted'".as_slice())
        );
        assert_eq!(config.get_ssl_mode(), PgSslMode::Require);

        let (config, _) = configure(
            "postgres://user:sslmode=disable@localhost/db?sslmode=verify%2Dfull&application_name=sslmode%3Ddisable",
            None,
        ).expect("encoded mode");
        assert_eq!(config.get_ssl_mode(), PgSslMode::Require);
        assert_eq!(config.get_password(), Some(b"sslmode=disable".as_slice()));
        assert_eq!(config.get_application_name(), Some("sslmode=disable"));

        let (config, connector) =
            configure("host=localhost password=sslmode=require", None).expect("password");
        assert_eq!(config.get_ssl_mode(), PgSslMode::Disable);
        assert!(matches!(connector, SslMode::NoTls(_)));
    }

    #[tokio::test]
    async fn tls_strict_modes_never_send_plaintext_when_server_refuses_tls() {
        // Cover each strict mode and input path once; policy combinations are
        // checked separately above.
        for (mode, source) in [
            ("require", "explicit"),
            ("verify-ca", "keyword"),
            ("verify-full", "url"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
            let port = listener.local_addr().expect("address").port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let mut request = [0; 8];
                socket.read_exact(&mut request).await.expect("SSL request");
                assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                socket.write_all(b"N").await.expect("refuse TLS");
                let mut byte = [0];
                let read = socket.read(&mut byte).await.expect("client disconnect");
                assert_eq!(read, 0, "client sent plaintext after TLS refusal");
            });
            let config = match source {
                "explicit" => PgConfig::new(
                    &format!("host=127.0.0.1 port={port} user=cdk sslmode=disable"),
                    Some(mode),
                    None,
                    None,
                ),
                "keyword" => PgConfig::from(
                    format!("host=127.0.0.1 port={port} user=cdk sslmode='{mode}'").as_str(),
                ),
                _ => PgConfig::from(
                    format!("postgres://cdk@127.0.0.1:{port}/db?sslmode={mode}").as_str(),
                ),
            };
            let stale = Arc::new(AtomicBool::new(false));
            let connection = PostgresConnection::new(config, Duration::from_secs(3), stale.clone());
            assert!(connection.inner().await.is_err(), "{source}: {mode}");
            timeout(Duration::from_secs(3), server)
                .await
                .expect("server completed")
                .expect("server task");
            assert!(stale.load(Ordering::Acquire));
        }
    }

    #[tokio::test]
    async fn tls_invalid_mode_fails_before_opening_a_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
        let port = listener.local_addr().expect("address").port();
        let config = PgConfig::new(
            &format!("host=127.0.0.1 port={port} user=cdk"),
            Some("requre"), // typos: ignore
            None,
            None,
        );
        let stale = Arc::new(AtomicBool::new(false));
        let connection = PostgresConnection::new(config, Duration::from_secs(3), stale.clone());
        let error = connection.inner().await.expect_err("invalid TLS mode");
        assert!(error.to_string().contains("Invalid PostgreSQL TLS mode"));
        assert!(stale.load(Ordering::Acquire));
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }
}
