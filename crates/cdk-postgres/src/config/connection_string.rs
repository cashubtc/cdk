//! Extract CDK extensions without rewriting other connection parameters.
//!
//! Keep ordinary driver parameters verbatim. In particular, whitespace and
//! strings resembling option names inside passwords must never be normalized.
use super::ConfigError;

type Parsed = (String, Option<String>, Option<String>);

fn driver_tls_mode(mode: &str) -> Result<&str, ConfigError> {
    match mode.to_ascii_lowercase().as_str() {
        "verify-ca" | "verify-full" => Ok("require"),
        "allow" => Ok("prefer"),
        "disable" => Ok("disable"),
        "prefer" => Ok("prefer"),
        "require" => Ok("require"),
        _ => Err(ConfigError::TlsMode),
    }
}

pub(super) fn parse(input: &str) -> Result<Parsed, ConfigError> {
    let input = input.trim_start();
    if input.starts_with("postgres://") || input.starts_with("postgresql://") {
        // Preserve the existing `postgres://... schema=...` constructor syntax.
        // Whitespace inside a URI itself must be percent-encoded.
        let (uri, suffix) = input.split_once(char::is_whitespace).unwrap_or((input, ""));
        let (remaining, suffix_schema, suffix_tls) = keywords(suffix)?;
        if !remaining.trim().is_empty() || suffix_tls.is_some() {
            return Err(ConfigError::ConnectionString);
        }
        let (url, schema, tls) = uri_parameters(uri)?;
        Ok((url, suffix_schema.or(schema), tls))
    } else {
        keywords(input)
    }
}

fn uri_parameters(uri: &str) -> Result<Parsed, ConfigError> {
    let Some((base, query)) = uri.split_once('?') else {
        return Ok((uri.to_owned(), None, None));
    };
    let mut schema = None;
    let mut tls = None;
    let mut parameters = Vec::new();
    for parameter in query.split('&').filter(|parameter| !parameter.is_empty()) {
        let (key, value) = parameter
            .split_once('=')
            .ok_or(ConfigError::ConnectionString)?;
        let decode = |s: &str| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .map(|s| s.into_owned())
                .map_err(|_| ConfigError::ConnectionString)
        };
        match decode(key)?.as_str() {
            "schema" => schema = Some(decode(value)?),
            "sslmode" => {
                let mode = decode(value)?;
                parameters.push(format!("sslmode={}", driver_tls_mode(&mode)?));
                tls = Some(mode);
            }
            _ => parameters.push(parameter.to_owned()),
        }
    }
    let url = match parameters.is_empty() {
        true => base.to_owned(),
        false => format!("{base}?{}", parameters.join("&")),
    };
    Ok((url, schema, tls))
}

fn keywords(mut input: &str) -> Result<Parsed, ConfigError> {
    let mut output = String::new();
    let mut schema = None;
    let mut tls = None;
    while !input.trim_start().is_empty() {
        input = input.trim_start();
        let end = input
            .find(|c: char| c == '=' || c.is_whitespace())
            .ok_or(ConfigError::ConnectionString)?;
        let key = &input[..end];
        let value = input[end..]
            .trim_start()
            .strip_prefix('=')
            .ok_or(ConfigError::ConnectionString)?
            .trim_start();
        // Locate the value boundary only; let tokio-postgres decode its quoting
        // and escaping rules instead of implementing a second value parser.
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
            "schema" | "sslmode" => {
                let decoded = format!("application_name={raw_value}")
                    .parse::<tokio_postgres::Config>()
                    .map_err(|_| ConfigError::ConnectionString)?
                    .get_application_name()
                    .ok_or(ConfigError::ConnectionString)?
                    .to_owned();
                if key == "schema" {
                    schema = Some(decoded);
                } else {
                    if !output.is_empty() {
                        output.push(' ');
                    }
                    output.push_str(&format!("sslmode={}", driver_tls_mode(&decoded)?));
                    tls = Some(decoded);
                }
            }
            _ => {
                if !output.is_empty() {
                    output.push(' ');
                }
                output.push_str(&input[..token_end]);
            }
        }
        input = &input[token_end..];
    }
    Ok((output, schema, tls))
}
