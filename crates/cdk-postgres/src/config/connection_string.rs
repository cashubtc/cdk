//! Extract the CDK `schema` extension and the TLS mode from a connection string.
//!
//! Every other driver parameter is preserved. In particular, whitespace and
//! text resembling option names inside passwords must never be rewritten.

use super::ConfigError;
use crate::tls::TlsPolicy;

/// A connection string with the CDK extensions removed.
#[derive(Debug, Default)]
pub(super) struct Parsed {
    /// Driver connection string, without `schema` and `sslmode`.
    pub(super) url: String,
    /// Decoded schema name.
    pub(super) schema: Option<String>,
    /// The schema name earlier releases used for the same setting, when it
    /// differs from the decoded name.
    pub(super) legacy_schema: Option<String>,
    /// TLS policy from the last `sslmode` parameter.
    pub(super) tls_policy: Option<TlsPolicy>,
}

fn has_uri_scheme(input: &str) -> bool {
    input.starts_with("postgres://") || input.starts_with("postgresql://")
}

pub(super) fn parse(input: &str) -> Result<Parsed, ConfigError> {
    let input = input.trim_start();
    if has_uri_scheme(input) {
        // Keep the existing `postgres://... schema=...` constructor syntax.
        // Whitespace inside a URI itself must be percent-encoded.
        let (uri, suffix) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
        let suffix = keywords(suffix)?;
        if !suffix.url.is_empty() || suffix.tls_policy.is_some() {
            return Err(ConfigError::ConnectionString);
        }
        let parsed = uri_parameters(uri)?;
        match suffix.schema {
            Some(schema) => Ok(Parsed {
                schema: Some(schema),
                legacy_schema: suffix.legacy_schema,
                ..parsed
            }),
            None => Ok(parsed),
        }
    } else {
        keywords(input)
    }
}

fn uri_parameters(uri: &str) -> Result<Parsed, ConfigError> {
    let (scheme, rest) = match uri.strip_prefix("postgres://") {
        Some(rest) => ("postgres://", rest),
        None => match uri.strip_prefix("postgresql://") {
            Some(rest) => ("postgresql://", rest),
            None => return Err(ConfigError::ConnectionString),
        },
    };

    // Follow RFC 3986 / libpq URI boundaries: `?` starts the query, `/`
    // before `?` starts the database path, and `@` only delimits credentials
    // inside the authority. Credentials containing `?`, `/`, or `@` must be
    // percent-encoded.
    let (hier_part, query) = match rest.split_once('?') {
        Some((hier_part, query)) => (hier_part, Some(query)),
        None => (rest, None),
    };
    let (authority, path) = match hier_part.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (hier_part, None),
    };
    if authority
        .split_once('@')
        .is_some_and(|(_, host)| host.contains('@'))
    {
        return Err(ConfigError::ConnectionString);
    }

    // tokio-postgres searches the entire URI for the first `@` when parsing
    // credentials. Percent-encoding `@` in the path and query keeps the driver
    // aligned with the authority boundary above, and tokio-postgres
    // percent-decodes `%40` back to `@` in both places.
    let mut url = String::with_capacity(uri.len());
    url.push_str(scheme);
    url.push_str(authority);
    if let Some(path) = path {
        url.push('/');
        push_escaping_at(&mut url, path);
    }

    let decode = |value: &str| {
        percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|_| ConfigError::ConnectionString)
    };
    let mut schema = None;
    let mut tls_policy = None;
    let mut has_query_param = false;
    if let Some(query) = query {
        for parameter in query.split('&').filter(|parameter| !parameter.is_empty()) {
            let (key, value) = parameter
                .split_once('=')
                .ok_or(ConfigError::ConnectionString)?;
            match decode(key)?.as_str() {
                "schema" => schema = Some(decode(value)?),
                "sslmode" => tls_policy = Some(TlsPolicy::parse(&decode(value)?)?),
                _ => {
                    url.push(match has_query_param {
                        true => '&',
                        false => '?',
                    });
                    push_escaping_at(&mut url, parameter);
                    has_query_param = true;
                }
            }
        }
    }

    // Earlier releases did not accept URI query parameters for the schema, so
    // there is no legacy name to look for.
    Ok(Parsed {
        url,
        schema,
        legacy_schema: None,
        tls_policy,
    })
}

fn push_escaping_at(output: &mut String, input: &str) {
    for (index, segment) in input.split('@').enumerate() {
        if index != 0 {
            output.push_str("%40");
        }
        output.push_str(segment);
    }
}

fn keywords(mut input: &str) -> Result<Parsed, ConfigError> {
    let mut output = String::new();
    let mut schema = None;
    let mut legacy_schema = None;
    let mut tls_policy = None;
    while !input.trim_start().is_empty() {
        input = input.trim_start();
        // A URI must not appear inside keyword syntax: stripping leading
        // `schema=` or `sslmode=` would otherwise leave the URI at the start of
        // `output`, causing tokio-postgres to switch to its URL parser without
        // our URI boundary checks or TLS policy extraction.
        if has_uri_scheme(input) {
            return Err(ConfigError::ConnectionString);
        }
        let end = input
            .find(|c: char| c == '=' || c.is_whitespace())
            .ok_or(ConfigError::ConnectionString)?;
        // tokio-postgres stops parsing without error on a bare leading `=`.
        if end == 0 {
            return Err(ConfigError::ConnectionString);
        }
        let key = &input[..end];
        let value = input[end..]
            .trim_start()
            .strip_prefix('=')
            .ok_or(ConfigError::ConnectionString)?
            .trim_start();
        // Find where the value ends only; leave decoding of its quotes and
        // escapes to tokio-postgres instead of keeping a second value parser.
        let quoted = value.starts_with('\'');
        let mut escaped = false;
        let mut end = value.len();
        for (index, ch) in value.char_indices().skip(usize::from(quoted)) {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if quoted && ch == '\'' {
                end = index + 1;
                break;
            } else if !quoted && ch.is_whitespace() {
                end = index;
                break;
            }
        }
        let raw_value = &value[..end];
        let token_end = input.len() - value.len() + end;
        match key {
            "schema" => {
                let decoded = decode_value(raw_value)?;
                legacy_schema = legacy_schema_name(raw_value, &decoded);
                schema = Some(decoded);
            }
            // Every occurrence is validated, including overridden ones, to
            // catch configuration typos. The policy is applied to the driver
            // configuration directly, so the parameter itself is dropped.
            "sslmode" => tls_policy = Some(TlsPolicy::parse(&decode_value(raw_value)?)?),
            _ => {
                if !output.is_empty() {
                    output.push(' ');
                }
                output.push_str(&input[..token_end]);
            }
        }
        input = &input[token_end..];
    }
    Ok(Parsed {
        url: output,
        schema,
        legacy_schema,
        tls_policy,
    })
}

/// Decode a keyword value with the driver's own quoting and escaping rules.
fn decode_value(raw_value: &str) -> Result<String, ConfigError> {
    format!("application_name={raw_value}")
        .parse::<tokio_postgres::Config>()
        .map_err(|_| ConfigError::ConnectionString)?
        .get_application_name()
        .map(str::to_owned)
        .ok_or(ConfigError::ConnectionString)
}

/// The schema name earlier releases derived from a raw `schema=` value.
///
/// They placed the raw text inside a quoted identifier without escaping it, so
/// quotes and backslashes became part of the name, and each `""` became `"`.
fn legacy_schema_name(raw_value: &str, decoded: &str) -> Option<String> {
    let legacy = raw_value.replace("\"\"", "\"");
    (legacy != decoded).then_some(legacy)
}
