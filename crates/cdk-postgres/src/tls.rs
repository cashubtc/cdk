//! Resolve connector verification and PostgreSQL negotiation from one TLS policy.

use cdk_common::database::Error;
use native_tls::{TlsConnector, TlsConnectorBuilder};
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::config::SslMode as PgSslMode;
use tokio_postgres::Config;

#[derive(Clone, Copy)]
enum TlsPolicy {
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

    fn build_connector<F>(self, build: F) -> Result<Option<MakeTlsConnector>, Error>
    where
        F: FnOnce(TlsConnectorBuilder) -> Result<TlsConnector, native_tls::Error>,
    {
        if matches!(self, Self::Disable) {
            return Ok(None);
        }

        let mut builder = TlsConnector::builder();
        builder.danger_accept_invalid_certs(matches!(self, Self::Prefer | Self::Require));
        builder.danger_accept_invalid_hostnames(!matches!(self, Self::VerifyFull));
        let connector = build(builder).map_err(|err| Error::Database(Box::new(err)))?;
        Ok(Some(MakeTlsConnector::new(connector)))
    }
}

pub(super) fn configure(
    config: &mut Config,
    mode: &str,
) -> Result<Option<MakeTlsConnector>, Error> {
    let policy = TlsPolicy::parse(mode)?;
    config.ssl_mode(policy.negotiation());
    policy.build_connector(|builder| builder.build())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk_sql_common::database::SqlBackend;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    use super::*;
    use crate::{PgConfig, PostgresBackend};

    fn configure(
        input: &str,
        explicit: Option<&str>,
    ) -> Result<(Config, Option<MakeTlsConnector>), Error> {
        PgConfig::new(input, explicit, None, None).driver_config()
    }

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
            assert_eq!(connector.is_none(), mode == "disable");
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
        assert!(connector.is_none());
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
            let backend = PostgresBackend::new(config).expect("valid TLS configuration");
            assert!(backend.acquire().await.is_err(), "{source}: {mode}");
            timeout(Duration::from_secs(3), server)
                .await
                .expect("server completed")
                .expect("server task");
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
        let error = PostgresBackend::new(config).expect_err("invalid TLS mode");
        assert!(error.to_string().contains("Invalid PostgreSQL TLS mode"));
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
    }
}
