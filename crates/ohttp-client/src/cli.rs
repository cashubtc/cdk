use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueHint};
use url::Url;

#[derive(Debug, Parser)]
#[command(
    version = env!("CARGO_PKG_VERSION"),
    about = "OHTTP Client",
    long_about = "Send arbitrary data through a generic OHTTP gateway for privacy protection",
)]
pub struct Cli {
    /// OHTTP gateway URL to connect to
    #[arg(
        long,
        env = "OHTTP_GATEWAY_URL",
        help = "URL of the OHTTP gateway (required for fetching keys)",
        value_hint = ValueHint::Url
    )]
    pub gateway_url: Option<Url>,

    /// OHTTP relay URL to connect to (optional, for routing requests through relay)
    #[arg(
        long,
        env = "OHTTP_RELAY_URL",
        help = "URL of the OHTTP relay (routes requests to gateway if specified)",
        value_hint = ValueHint::Url
    )]
    pub relay_url: Option<Url>,

    /// Target gateway URL when using a relay (optional, uses relay's configured gateway if not specified)
    #[arg(
        long,
        env = "OHTTP_RELAY_GATEWAY_URL",
        help = "Target gateway URL when using a relay (overrides relay's default gateway)",
        value_hint = ValueHint::Url,
        requires = "relay_url"
    )]
    pub relay_gateway_url: Option<Url>,

    /// OHTTP keys file to use for encryption
    #[arg(
        long,
        help = "OHTTP keys file (if not provided, will attempt to fetch from gateway or relay)",
        value_hint = ValueHint::FilePath
    )]
    pub ohttp_keys: Option<PathBuf>,

    /// Additional headers to include in requests (format: 'Header: Value')
    #[arg(
        long,
        help = "Additional headers to include (can be specified multiple times)",
        value_parser = parse_header
    )]
    pub header: Vec<(String, String)>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Send arbitrary data through the OHTTP gateway to its configured backend
    Send {
        /// HTTP method to use (default: POST)
        #[arg(long, default_value = "POST")]
        method: String,

        /// Data to send in the request body (optional for GET requests)
        #[arg(short, long, help = "Data to send in request body")]
        data: Option<String>,

        /// Read data from a file
        #[arg(
            short,
            long,
            help = "Read data from file instead of command line",
            value_hint = ValueHint::FilePath
        )]
        file: Option<PathBuf>,

        /// Send JSON data (will set Content-Type: application/json)
        #[arg(long, help = "Send JSON data (sets Content-Type header automatically)")]
        json: Option<String>,

        /// Request path (default: /)
        #[arg(
            long = "path",
            help = "Request path to send to (will be forwarded to backend)",
            default_value = "/"
        )]
        request_path: String,
    },

    /// Fetch OHTTP keys from the gateway
    GetKeys,

    /// Send a health check to the gateway
    Health,

    /// Show current configuration and available endpoints
    Info,
}

/// Parse header argument in the format "Header: Value"
fn parse_header(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err("Header must be in format 'Header: Value'".to_string());
    }

    let header = parts[0].trim().to_string();
    let value = parts[1].trim().to_string();

    if header.is_empty() {
        return Err("Header name cannot be empty".to_string());
    }

    Ok((header, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_valid() {
        let result = parse_header("Content-Type: application/json");
        assert_eq!(
            result,
            Ok(("Content-Type".to_string(), "application/json".to_string()))
        );
    }

    #[test]
    fn test_parse_header_no_value() {
        let result = parse_header("Content-Type:");
        assert_eq!(result, Ok(("Content-Type".to_string(), "".to_string())));
    }

    #[test]
    fn test_parse_header_empty() {
        let result = parse_header("");
        assert_eq!(
            result,
            Err("Header must be in format 'Header: Value'".to_string())
        );
    }

    #[test]
    fn test_parse_header_no_colon() {
        let result = parse_header("Content-Type");
        assert_eq!(
            result,
            Err("Header must be in format 'Header: Value'".to_string())
        );
    }
}
