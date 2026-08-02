use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Result};

use crate::config::{Iroh, IrohDiscovery};

const ENV_ENABLED: &str = "CDK_MINTD_IROH_ENABLED";
const ENV_SECRET_KEY_FILE: &str = "CDK_MINTD_IROH_SECRET_KEY_FILE";
const ENV_ENDPOINT_TICKET_FILE: &str = "CDK_MINTD_IROH_ENDPOINT_TICKET_FILE";
const ENV_GENERATE_SECRET_KEY: &str = "CDK_MINTD_IROH_GENERATE_SECRET_KEY";
const ENV_DISCOVERY: &str = "CDK_MINTD_IROH_DISCOVERY";
const ENV_RELAY_URLS: &str = "CDK_MINTD_IROH_RELAY_URLS";
const ENV_STATIC_TICKETS: &str = "CDK_MINTD_IROH_STATIC_TICKETS";
const ENV_BIND_ADDR: &str = "CDK_MINTD_IROH_BIND_ADDR";
const ENV_CONNECT_TIMEOUT: &str = "CDK_MINTD_IROH_CONNECT_TIMEOUT_SECONDS";
const ENV_STREAM_OPEN_TIMEOUT: &str = "CDK_MINTD_IROH_STREAM_OPEN_TIMEOUT_SECONDS";
const ENV_HEADERS_TIMEOUT: &str = "CDK_MINTD_IROH_HEADERS_TIMEOUT_SECONDS";
const ENV_BODY_PROGRESS_TIMEOUT: &str = "CDK_MINTD_IROH_BODY_PROGRESS_TIMEOUT_SECONDS";
const ENV_SHUTDOWN_TIMEOUT: &str = "CDK_MINTD_IROH_SHUTDOWN_TIMEOUT_SECONDS";
const ENV_MAX_CONNECTIONS: &str = "CDK_MINTD_IROH_MAX_CONNECTIONS";
const ENV_MAX_POOLED_CONNECTIONS: &str = "CDK_MINTD_IROH_MAX_POOLED_CONNECTIONS";
const ENV_MAX_CONNECTIONS_PER_PEER: &str = "CDK_MINTD_IROH_MAX_CONNECTIONS_PER_PEER";
const ENV_MAX_STREAMS: &str = "CDK_MINTD_IROH_MAX_STREAMS";
const ENV_MAX_STREAMS_PER_CONNECTION: &str = "CDK_MINTD_IROH_MAX_STREAMS_PER_CONNECTION";
const ENV_MAX_HEADER_BYTES: &str = "CDK_MINTD_IROH_MAX_HEADER_BYTES";
const ENV_MAX_REQUEST_BODY_BYTES: &str = "CDK_MINTD_IROH_MAX_REQUEST_BODY_BYTES";
const ENV_MAX_RESPONSE_BODY_BYTES: &str = "CDK_MINTD_IROH_MAX_RESPONSE_BODY_BYTES";

const IROH_ENV_VARS: &[&str] = &[
    ENV_ENABLED,
    ENV_SECRET_KEY_FILE,
    ENV_ENDPOINT_TICKET_FILE,
    ENV_GENERATE_SECRET_KEY,
    ENV_DISCOVERY,
    ENV_RELAY_URLS,
    ENV_STATIC_TICKETS,
    ENV_BIND_ADDR,
    ENV_CONNECT_TIMEOUT,
    ENV_STREAM_OPEN_TIMEOUT,
    ENV_HEADERS_TIMEOUT,
    ENV_BODY_PROGRESS_TIMEOUT,
    ENV_SHUTDOWN_TIMEOUT,
    ENV_MAX_CONNECTIONS,
    ENV_MAX_POOLED_CONNECTIONS,
    ENV_MAX_CONNECTIONS_PER_PEER,
    ENV_MAX_STREAMS,
    ENV_MAX_STREAMS_PER_CONNECTION,
    ENV_MAX_HEADER_BYTES,
    ENV_MAX_REQUEST_BODY_BYTES,
    ENV_MAX_RESPONSE_BODY_BYTES,
];

pub(super) fn iroh_env_configured() -> bool {
    IROH_ENV_VARS.iter().any(|name| env::var_os(name).is_some())
}

impl Iroh {
    pub(super) fn apply_env(mut self) -> Result<Self> {
        if let Some(value) = parse_env(ENV_ENABLED)? {
            self.enabled = value;
        }
        if let Ok(value) = env::var(ENV_SECRET_KEY_FILE) {
            self.secret_key_file = Some(PathBuf::from(value));
        }
        if let Ok(value) = env::var(ENV_ENDPOINT_TICKET_FILE) {
            self.endpoint_ticket_file = Some(PathBuf::from(value));
        }
        if let Some(value) = parse_env(ENV_GENERATE_SECRET_KEY)? {
            self.generate_secret_key = value;
        }
        if let Ok(value) = env::var(ENV_DISCOVERY) {
            self.discovery = match value.to_ascii_lowercase().as_str() {
                "n0" => IrohDiscovery::N0,
                "static" => IrohDiscovery::Static,
                "custom" => IrohDiscovery::Custom,
                _ => return Err(anyhow!("invalid value in {ENV_DISCOVERY}")),
            };
        }
        if let Ok(value) = env::var(ENV_RELAY_URLS) {
            self.relay_urls = parse_list(&value);
        }
        if let Ok(value) = env::var(ENV_STATIC_TICKETS) {
            self.static_tickets = parse_list(&value);
        }
        if let Some(value) = parse_env(ENV_BIND_ADDR)? {
            self.bind_addr = Some(value);
        }
        assign(ENV_CONNECT_TIMEOUT, &mut self.timeouts.connect_seconds)?;
        assign(
            ENV_STREAM_OPEN_TIMEOUT,
            &mut self.timeouts.stream_open_seconds,
        )?;
        assign(ENV_HEADERS_TIMEOUT, &mut self.timeouts.headers_seconds)?;
        assign(
            ENV_BODY_PROGRESS_TIMEOUT,
            &mut self.timeouts.body_progress_seconds,
        )?;
        assign(ENV_SHUTDOWN_TIMEOUT, &mut self.timeouts.shutdown_seconds)?;
        assign(ENV_MAX_CONNECTIONS, &mut self.limits.max_connections)?;
        assign(
            ENV_MAX_POOLED_CONNECTIONS,
            &mut self.limits.max_pooled_connections,
        )?;
        assign(
            ENV_MAX_CONNECTIONS_PER_PEER,
            &mut self.limits.max_connections_per_peer,
        )?;
        assign(ENV_MAX_STREAMS, &mut self.limits.max_streams)?;
        assign(
            ENV_MAX_STREAMS_PER_CONNECTION,
            &mut self.limits.max_streams_per_connection,
        )?;
        assign(ENV_MAX_HEADER_BYTES, &mut self.limits.max_header_bytes)?;
        assign(
            ENV_MAX_REQUEST_BODY_BYTES,
            &mut self.limits.max_request_body_bytes,
        )?;
        assign(
            ENV_MAX_RESPONSE_BODY_BYTES,
            &mut self.limits.max_response_body_bytes,
        )?;
        Ok(self)
    }
}

fn parse_env<T>(name: &str) -> Result<Option<T>>
where
    T: FromStr,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| anyhow!("invalid value in {name}")),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(anyhow!("non-Unicode value in {name}")),
    }
}

fn assign<T>(name: &str, target: &mut T) -> Result<()>
where
    T: FromStr,
{
    if let Some(value) = parse_env(name)? {
        *target = value;
    }
    Ok(())
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}
