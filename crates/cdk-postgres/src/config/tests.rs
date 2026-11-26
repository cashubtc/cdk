use std::error::Error as StdError;

use tokio_postgres::config::{Host, SslMode as PgSslMode};

use super::*;

fn assert_error_hides_secret(error: &Error, secret: &str, context: &str) {
    assert!(!format!("{error:?}").contains(secret), "{context}");
    assert!(!error.to_string().contains(secret), "{context}");
    let mut source = StdError::source(error);
    while let Some(cause) = source {
        assert!(!format!("{cause:?}").contains(secret), "{context}");
        assert!(!cause.to_string().contains(secret), "{context}");
        source = cause.source();
    }
}

#[test]
fn keyword_credentials_and_application_names_are_preserved() {
    for (input, password) in [
        ("password='sslmode=verify-full'", "sslmode=verify-full"),
        (
            "password='a  schema=private sslmode=disable  b'",
            "a  schema=private sslmode=disable  b",
        ),
        (r"password=a\ b\ schema=private", "a b schema=private"),
        (r"password='a\'b\\c'", "a'b\\c"),
    ] {
        let config = PgConfig::from(
            format!(
                "host=localhost {input} application_name='sslmode=verify-ca' \
                 sslmode='require' schema='my schema'"
            )
            .as_str(),
        );
        let (driver, tls) = config.driver_config().expect("valid configuration");
        assert_eq!(driver.get_password(), Some(password.as_bytes()));
        assert_eq!(driver.get_application_name(), Some("sslmode=verify-ca"));
        assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);
        assert!(matches!(tls, SslMode::NativeTls(_)));
        assert_eq!(config.schema.as_deref(), Some("my schema"));
    }
}

#[test]
fn uri_credentials_and_encoded_parameters_are_preserved() {
    for uri in [
        "postgres://user:sslmode=verify-full@localhost/db?sslmode=verify%2Dfull&schema=a%20b%2Bc",
        "postgresql://user:sslmode%3Dverify-full@localhost/db?%73slmode=verify-full&schema=a%20b%2Bc",
    ] {
        let config = PgConfig::from(uri);
        let (driver, _) = config.driver_config().expect("valid configuration");
        assert_eq!(driver.get_password(), Some(b"sslmode=verify-full".as_slice()));
        assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);
        assert_eq!(config.schema.as_deref(), Some("a b+c"));
    }
    for (uri, expected_password) in [
        (
            "postgres://user:pa%3Fss@localhost/db?sslmode=require",
            b"pa?ss".as_slice(),
        ),
        (
            "postgres://user:p%40s%2Fs@localhost/db?sslmode=require",
            b"p@s/s".as_slice(),
        ),
    ] {
        let config = PgConfig::from(uri);
        let (driver, _) = config.driver_config().expect("valid configuration");
        assert_eq!(driver.get_user(), Some("user"), "{uri}");
        assert_eq!(driver.get_password(), Some(expected_password), "{uri}");
        assert_eq!(
            driver.get_hosts(),
            &[Host::Tcp("localhost".to_owned())],
            "{uri}"
        );
        assert_eq!(driver.get_dbname(), Some("db"), "{uri}");
        assert_eq!(driver.get_ssl_mode(), PgSslMode::Require, "{uri}");
    }
    let config = PgConfig::from("postgres://localhost/db? schema='legacy schema'");
    assert!(config.driver_config().is_ok());
    assert_eq!(config.schema.as_deref(), Some("legacy schema"));
}

#[test]
fn uri_at_signs_in_path_and_query_do_not_act_as_credentials() {
    for (uri, expected_db) in [
        (
            "postgres://localhost/db?sslmode=require&schema=ops@blue",
            Some("db"),
        ),
        ("postgres://localhost?sslmode=require&schema=ops@blue", None),
        (
            "postgres://localhost:5432?sslmode=require&schema=ops@blue",
            None,
        ),
    ] {
        let config = PgConfig::from(uri);
        let (driver, _) = config.driver_config().expect("valid configuration");
        assert_eq!(
            driver.get_hosts(),
            &[Host::Tcp("localhost".to_owned())],
            "{uri}"
        );
        assert_eq!(driver.get_user(), None, "{uri}");
        assert_eq!(driver.get_dbname(), expected_db, "{uri}");
        assert_eq!(driver.get_ssl_mode(), PgSslMode::Require, "{uri}");
        assert_eq!(config.schema.as_deref(), Some("ops@blue"), "{uri}");
    }

    for (uri, expected_db, expected_schema) in [
        (
            "postgres://localhost/db?sslmode=require&application_name=ops@blue",
            Some("db"),
            None,
        ),
        (
            "postgres://localhost?sslmode=require&application_name=ops@blue",
            None,
            None,
        ),
        (
            "postgres://localhost/db?sslmode=require&schema=ops@blue&application_name=ops@blue",
            Some("db"),
            Some("ops@blue"),
        ),
    ] {
        let config = PgConfig::from(uri);
        let (driver, _) = config.driver_config().expect("valid configuration");
        assert_eq!(
            driver.get_hosts(),
            &[Host::Tcp("localhost".to_owned())],
            "{uri}"
        );
        assert_eq!(driver.get_user(), None, "{uri}");
        assert_eq!(driver.get_dbname(), expected_db, "{uri}");
        assert_eq!(driver.get_application_name(), Some("ops@blue"), "{uri}");
        assert_eq!(driver.get_ssl_mode(), PgSslMode::Require, "{uri}");
        assert_eq!(config.schema.as_deref(), expected_schema, "{uri}");
    }

    let config = PgConfig::from("postgres://localhost/db?user=alice@example.com&sslmode=require");
    let (driver, _) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_hosts(), &[Host::Tcp("localhost".to_owned())]);
    assert_eq!(driver.get_user(), Some("alice@example.com"));
    assert_eq!(driver.get_dbname(), Some("db"));
    assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);

    for (uri, expected_ssl) in [
        (
            "postgres://localhost/db@prod?sslmode=require",
            PgSslMode::Require,
        ),
        ("postgres://localhost/db@prod", PgSslMode::Disable),
    ] {
        let config = PgConfig::from(uri);
        let (driver, _) = config.driver_config().expect("valid configuration");
        assert_eq!(
            driver.get_hosts(),
            &[Host::Tcp("localhost".to_owned())],
            "{uri}"
        );
        assert_eq!(driver.get_user(), None, "{uri}");
        assert_eq!(driver.get_dbname(), Some("db@prod"), "{uri}");
        assert_eq!(driver.get_ssl_mode(), expected_ssl, "{uri}");
    }

    let config =
        PgConfig::from("postgres://localhost/db?sslmode=require&schema=ops@blue&options=a?b");
    let (driver, _) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_options(), Some("a?b"));
    assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);
    assert_eq!(config.schema.as_deref(), Some("ops@blue"));

    let config = PgConfig::from("postgres://localhost/db?application_name=a%40b%2540c");
    let (driver, _) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_application_name(), Some("a@b%40c"));
}

#[test]
fn uri_unencoded_delimiters_in_credentials_are_rejected() {
    let baseline = PgConfig::from("host=localhost");
    for uri in [
        "postgres://user:secret?ss@localhost/db?sslmode=require",
        "postgres://user:secret?ss@localhost/db",
        "postgres://user?secret@localhost/db?sslmode=require",
        "postgres://user?secret=1@localhost/db?sslmode=require",
        "postgres://user:5432?secret=1@localhost/db",
        "postgres://user:secret/ss@localhost/db?sslmode=require",
        "postgres://user:secret@host@localhost/db?sslmode=require",
    ] {
        let config = PgConfig::from(uri);
        let error = config.driver_config().expect_err("unencoded delimiter");
        assert_error_hides_secret(&error, "secret", uri);
        assert!(config.has_same_tls_policy(&baseline).is_err(), "{uri}");
    }

    let error = PgConfig::from("postgres://localhost/db?sslmode=requre&schema=ops@blue") // typos: ignore
        .validate()
        .expect_err("invalid TLS mode");
    assert_eq!(error.to_string(), "Invalid PostgreSQL TLS mode");
}

#[test]
fn tls_precedence_and_duplicate_parameters_match_driver_behavior() {
    let config = PgConfig::new(
        "host=localhost sslmode = 'prefer' sslmode=require",
        Some("disable"),
        None,
        None,
    );
    let (driver, tls) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_ssl_mode(), PgSslMode::Disable);
    assert!(matches!(tls, SslMode::NoTls(_)));
    let config = PgConfig::from("host=localhost sslmode=disable sslmode='require'");
    let (driver, _) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);
}

#[test]
fn invalid_tls_mode_in_the_connection_string_reports_the_tls_error() {
    for input in [
        "host=localhost sslmode=requre",          // typos: ignore
        "postgres://localhost/db?sslmode=requre", // typos: ignore
    ] {
        for explicit in [None, Some("require")] {
            let error = PgConfig::new(input, explicit, None, None)
                .validate()
                .expect_err("invalid TLS mode");
            assert_eq!(error.to_string(), "Invalid PostgreSQL TLS mode", "{input}");
        }
    }
}

#[test]
fn invalid_extensions_cannot_inject_driver_parameters_or_leak_credentials() {
    let baseline = PgConfig::from("host=localhost");
    for input in [
        "password=secret schema='unterminated",
        "password=secret sslmode='disable user=attacker'",
        "postgres://user:secret@localhost/db?schema=%FF",
        "password=secret schema=",
        "postgres://user:secret@localhost/db sslmode=disable",
        "password='secret",
        "host=localhost password='abc'secret=1'",
        "schema=mint postgres://user:secret@localhost/db?sslmode=require",
        "schema=mint postgresql://user:secret@localhost/db?sslmode=prefer",
        "schema=mint postgres://user:secret@localhost/db",
        "schema=mint postgres://user:secret@localhost/db?application_name=ops@evil.com",
        "sslmode=disable postgres://user:secret@localhost/db?sslmode=require",
        "host=localhost =secret sslmode=require",
        "host=localhost =foo password=secret sslmode=require",
        "host='localhost'=secret",
        "=secret",
    ] {
        for explicit in [None, Some("disable")] {
            let config = PgConfig::new(input, explicit, None, None);
            let error = config.driver_config().expect_err("invalid configuration");
            assert_error_hides_secret(&error, "secret", input);
            assert!(!format!("{config:?}").contains("secret"), "{input}");
            assert!(config.has_same_tls_policy(&baseline).is_err(), "{input}");
        }
    }

    // URI schemes inside keyword values remain valid values.
    let config = PgConfig::from(
        "host=localhost password='postgres://secret' application_name=postgresql://app sslmode=require",
    );
    let (driver, _) = config.driver_config().expect("valid configuration");
    assert_eq!(driver.get_password(), Some(b"postgres://secret".as_slice()));
    assert_eq!(driver.get_application_name(), Some("postgresql://app"));
    assert_eq!(driver.get_ssl_mode(), PgSslMode::Require);
}

#[test]
fn keyword_password_decoding_matches_the_driver_at_boundaries() {
    for input in [
        r"password=trailing\",
        r"password=escaped\ space",
        "password=''",
        "password=' Unicode 密碼  '",
        "password='one' application_name=two",
    ] {
        let original: tokio_postgres::Config = input.parse().expect("driver parses input");
        let (adapted, _) = PgConfig::from(input)
            .driver_config()
            .expect("valid configuration");
        assert_eq!(adapted.get_password(), original.get_password());
        assert_eq!(
            adapted.get_application_name(),
            original.get_application_name()
        );
    }
}

#[test]
fn quoted_or_escaped_schema_records_the_legacy_literal_name() {
    for (input, schema, legacy) in [
        ("host=localhost schema=mint", "mint", None),
        ("host=localhost schema='mint'", "mint", Some("'mint'")),
        (r"host=localhost schema=mi\nt", "mint", Some(r"mi\nt")),
        // Earlier releases read each `""` inside the quoted identifier as `"`.
        (r#"host=localhost schema=a""b"#, r#"a""b"#, Some(r#"a"b"#)),
        (
            r#"host=localhost schema='a""b'"#,
            r#"a""b"#,
            Some(r#"'a"b'"#),
        ),
        (r#"host=localhost schema=a"b"#, r#"a"b"#, None),
        (
            "postgres://localhost/db schema='mint'",
            "mint",
            Some("'mint'"),
        ),
        // Earlier releases rejected schema query parameters, so no legacy name.
        ("postgres://localhost/db?schema=%27mint%27", "'mint'", None),
    ] {
        let config = PgConfig::from(input);
        assert_eq!(config.schema.as_deref(), Some(schema), "{input}");
        assert_eq!(config.legacy_schema.as_deref(), legacy, "{input}");
    }
}
