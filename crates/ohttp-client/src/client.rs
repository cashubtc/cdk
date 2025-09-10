use anyhow::{anyhow, Result};
use base64::Engine;
use http::HeaderMap;
use reqwest::Client;
use tracing::{debug, info};
use url::Url;

use crate::cli::{Cli, Commands};

/// OHTTP client for sending requests through gateways or relays
pub struct OhttpClient {
    client: Client,
    target_url: Url,
    is_relay: bool,
    relay_gateway_url: Option<Url>,
    ohttp_keys: Option<Vec<u8>>,
    keys_source_url: Url,
}

impl OhttpClient {
    /// Create a new OHTTP client
    pub fn new(
        target_url: Url,
        is_relay: bool,
        relay_gateway_url: Option<Url>,
        ohttp_keys: Option<Vec<u8>>,
        keys_source_url: Url,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            target_url,
            is_relay,
            relay_gateway_url,
            ohttp_keys,
            keys_source_url,
        }
    }

    /// Fetch OHTTP keys from the keys source (can be different from target URL)
    pub async fn fetch_keys(&self) -> Result<Vec<u8>> {
        let keys_url = self.keys_source_url.join("/ohttp-keys")?;

        debug!("Fetching OHTTP keys from: {}", keys_url);

        let response = self.client.get(keys_url).send().await?.error_for_status()?;

        let keys = response.bytes().await?;
        debug!("Fetched OHTTP keys, size: {} bytes", keys.len());

        Ok(keys.to_vec())
    }

    /// Send a request through OHTTP to the target's backend
    pub async fn send_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<OhttpResponse> {
        // Use proper OHTTP encapsulation
        self.send_ohttp_request(method, body, headers, request_path)
            .await
    }

    /// Send a request using proper OHTTP encapsulation
    pub async fn send_ohttp_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<OhttpResponse> {
        // Fetch OHTTP keys if not already available
        let keys_data = if let Some(keys) = &self.ohttp_keys {
            keys.clone()
        } else {
            self.fetch_keys().await?
        };

        // Parse the OHTTP keys and create client request
        let client_request = ohttp::ClientRequest::from_encoded_config(&keys_data)
            .map_err(|e| anyhow!("Failed to decode OHTTP keys: {}", e))?;

        debug!("Created OHTTP client request");

        // Create BHTTP request
        let bhttp_request = self.create_bhttp_request(method, body, headers, request_path)?;
        debug!("Created BHTTP request, size: {} bytes", bhttp_request.len());

        // Encapsulate the request using OHTTP
        let (ohttp_request, response_context) = client_request
            .encapsulate(&bhttp_request)
            .map_err(|e| anyhow!("Failed to encapsulate OHTTP request: {}", e))?;

        debug!(
            "Encapsulated OHTTP request, size: {} bytes",
            ohttp_request.len()
        );

        // Send directly to the target URL without appending .well-known/ohttp-gateway
        let endpoint_url = self.target_url.clone();

        debug!("Sending OHTTP request to: {}", endpoint_url);

        // Send the OHTTP request
        let start_time = std::time::Instant::now();
        let response = self
            .client
            .post(endpoint_url)
            .header("content-type", "message/ohttp-req")
            .body(ohttp_request)
            .send()
            .await?;

        let elapsed = start_time.elapsed();

        debug!(
            "OHTTP response received in {:.2}ms: {} {}",
            elapsed.as_millis(),
            response.status(),
            response.url()
        );

        // Check if we got the expected content type
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .unwrap_or("");

        if content_type != "message/ohttp-res" {
            debug!("Warning: Unexpected content type: {}", content_type);
        }

        let _response_status = response.status().as_u16();
        let _response_headers = response.headers().clone();
        let ohttp_response_body = response.bytes().await?;

        debug!(
            "OHTTP response body size: {} bytes",
            ohttp_response_body.len()
        );

        // Decapsulate the OHTTP response
        let bhttp_response = response_context
            .decapsulate(&ohttp_response_body)
            .map_err(|e| anyhow!("Failed to decapsulate OHTTP response: {}", e))?;

        debug!(
            "Decapsulated BHTTP response, size: {} bytes",
            bhttp_response.len()
        );

        // Parse the BHTTP response
        let (status, headers, body) = self.parse_bhttp_response(&bhttp_response)?;

        Ok(OhttpResponse {
            status,
            headers,
            body,
            elapsed,
        })
    }

    /// Create a BHTTP request from the given parameters
    fn create_bhttp_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<Vec<u8>> {
        use bhttp::Message;

        debug!("Creating BHTTP request: {} {}", method, request_path);

        // Create the BHTTP message
        let mut bhttp_msg = Message::request(
            method.as_bytes().to_vec(),
            b"https".to_vec(),                // scheme
            b"backend.example.com".to_vec(),  // authority (will be overridden by gateway)
            request_path.as_bytes().to_vec(), // path
        );

        // Add headers
        for (name, value) in headers {
            bhttp_msg.put_header(name.as_bytes(), value.as_bytes());
            debug!("Added header: {}: {}", name, value);
        }

        // Add body
        if !body.is_empty() {
            bhttp_msg.write_content(body);
            debug!("Added body, size: {} bytes", body.len());
        }

        // Serialize to bytes
        let mut bhttp_bytes = Vec::new();
        bhttp_msg
            .write_bhttp(bhttp::Mode::KnownLength, &mut bhttp_bytes)
            .map_err(|e| anyhow!("Failed to write BHTTP request: {}", e))?;

        Ok(bhttp_bytes)
    }

    /// Parse a BHTTP response into status, headers, and body
    fn parse_bhttp_response(&self, bhttp_bytes: &[u8]) -> Result<(u16, HeaderMap, Vec<u8>)> {
        use bhttp::Message;

        debug!("Parsing BHTTP response, size: {} bytes", bhttp_bytes.len());

        let mut cursor = std::io::Cursor::new(bhttp_bytes);
        let bhttp_msg = Message::read_bhttp(&mut cursor)
            .map_err(|e| anyhow!("Failed to read BHTTP response: {}", e))?;

        // Extract status
        let status = bhttp_msg
            .control()
            .status()
            .ok_or_else(|| anyhow!("Missing status in BHTTP response"))?;

        debug!("Parsed status: {}", u16::from(status));

        // Extract headers
        let mut headers = HeaderMap::new();
        for field in bhttp_msg.header().fields() {
            let name = String::from_utf8_lossy(field.name());
            let value = String::from_utf8_lossy(field.value());

            if let (Ok(header_name), Ok(header_value)) = (
                http::HeaderName::from_bytes(field.name()),
                http::HeaderValue::from_bytes(field.value()),
            ) {
                headers.insert(header_name, header_value);
                debug!("Parsed header: {}: {}", name, value);
            }
        }

        // Extract body
        let body = bhttp_msg.content().to_vec();
        debug!("Parsed body, size: {} bytes", body.len());

        Ok((status.into(), headers, body))
    }

    /// Send a raw HTTP request to the target (gateway or relay) (for testing/migration to full OHTTP)
    pub async fn send_relay_compatible_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<OhttpResponse> {
        if self.is_relay {
            self.send_relay_request(method, body, headers, request_path)
                .await
        } else {
            self.send_gateway_request(method, body, headers, request_path)
                .await
        }
    }

    /// Send a raw HTTP request to a gateway
    pub async fn send_gateway_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<OhttpResponse> {
        // Create a URL for the gateway test endpoint
        let mut gateway_endpoint = self.target_url.clone();
        gateway_endpoint.set_path("/test-gateway");

        debug!(
            "Sending {} request to gateway test endpoint: {}",
            method, gateway_endpoint
        );
        debug!("Forwarding to backend path: {}", request_path);

        // Create the inner request as JSON for testing
        let body_b64 = base64::engine::general_purpose::STANDARD.encode(body);
        let inner_request = serde_json::json!({
            "method": method,
            "path": request_path,
            "headers": headers,
            "body": body_b64
        });

        let request_body = serde_json::to_string(&inner_request)?;

        let mut request_builder = self.client.post(gateway_endpoint);
        request_builder = request_builder.header("content-type", "application/json");

        let start_time = std::time::Instant::now();

        let response = request_builder.body(request_body).send().await?;
        let elapsed = start_time.elapsed();

        debug!(
            "Gateway response received in {:.2}ms: {} {}",
            elapsed.as_millis(),
            response.status(),
            response.url()
        );

        // Get response text
        let _status = response.status();
        let _headers = response.headers().clone();
        let body = response.bytes().await?;

        debug!("Gateway response body size: {} bytes", body.len());

        // Parse the JSON response from the test gateway
        let json_str = std::str::from_utf8(&body)?;
        let json_resp: serde_json::Value = serde_json::from_str(json_str)?;

        // Extract the actual response data
        let actual_status = json_resp["status"].as_u64().unwrap_or(500) as u16;
        let actual_body_b64 = json_resp["body"].as_str().unwrap_or("");

        let actual_body = base64::engine::general_purpose::STANDARD.decode(actual_body_b64)?;

        Ok(OhttpResponse {
            status: actual_status,
            headers: HeaderMap::new(), // Initialize empty headers
            body: actual_body,
            elapsed,
        })
    }

    /// Send a request to a relay with optional custom gateway override
    pub async fn send_relay_request(
        &self,
        method: &str,
        body: &[u8],
        headers: &[(String, String)],
        request_path: &str,
    ) -> Result<OhttpResponse> {
        // For relays, the request path can include the target gateway URL
        let relay_path = if let Some(gateway_url) = &self.relay_gateway_url {
            // Prepend the gateway URL to the path
            format!("/{}{}", gateway_url, request_path)
        } else {
            // Use the original path (relay will use its configured default gateway)
            request_path.to_string()
        };

        debug!(
            "Sending {} request to relay endpoint: {} with path: {}",
            method, self.target_url, relay_path
        );

        // For now, we send requests to the relay as JSON for testing
        // In a real OHTTP implementation, this should use proper OHTTP encapsulation
        let body_b64 = base64::engine::general_purpose::STANDARD.encode(body);
        let inner_request = serde_json::json!({
            "method": method,
            "path": relay_path,
            "headers": headers,
            "body": body_b64
        });

        let request_body = serde_json::to_string(&inner_request)?;
        let relay_endpoint = self.target_url.join("/test-gateway")?; // Use test endpoint for now

        let mut request_builder = self.client.post(relay_endpoint);
        request_builder = request_builder.header("content-type", "application/json");

        let start_time = std::time::Instant::now();

        let response = request_builder.body(request_body).send().await?;
        let elapsed = start_time.elapsed();

        debug!(
            "Relay response received in {:.2}ms: {} {}",
            elapsed.as_millis(),
            response.status(),
            response.url()
        );

        // Get response text
        let _status = response.status();
        let _headers = response.headers().clone();
        let body = response.bytes().await?;

        debug!("Relay response body size: {} bytes", body.len());

        // Parse the JSON response from the relay
        let json_str = std::str::from_utf8(&body)?;
        let json_resp: serde_json::Value = serde_json::from_str(json_str)?;

        // Extract the actual response data
        let actual_status = json_resp["status"].as_u64().unwrap_or(500) as u16;
        let actual_body_b64 = json_resp["body"].as_str().unwrap_or("");

        let actual_body = base64::engine::general_purpose::STANDARD.decode(actual_body_b64)?;

        Ok(OhttpResponse {
            status: actual_status,
            headers: HeaderMap::new(), // Initialize empty headers
            body: actual_body,
            elapsed,
        })
    }

    /// Get target information
    pub async fn get_target_info(&self) -> Result<TargetInfo> {
        let keys = self.fetch_keys().await?;

        Ok(TargetInfo {
            target_url: self.target_url.clone(),
            is_relay: self.is_relay,
            keys_available: true,
            keys_size: keys.len(),
        })
    }

    /// Send health check
    pub async fn health_check(&self) -> Result<OhttpResponse> {
        let start_time = std::time::Instant::now();
        let response = self.client.get(self.target_url.as_str()).send().await?;
        let elapsed = start_time.elapsed();

        Ok(OhttpResponse {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.bytes().await?.to_vec(),
            elapsed,
        })
    }
}

#[derive(Debug)]
pub struct OhttpResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub elapsed: std::time::Duration,
}

impl OhttpResponse {
    /// Get response body as text
    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.clone())
            .map_err(|e| anyhow!("Failed to decode response as UTF-8: {}", e))
    }

    /// Get response body as JSON
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| anyhow!("Failed to parse JSON response: {}", e))
    }

    /// Check if response is JSON
    pub fn is_json(&self) -> bool {
        self.headers
            .get("content-type")
            .and_then(|ct| ct.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false)
    }
}

#[derive(Debug)]
pub struct GatewayInfo {
    pub gateway_url: Url,
    pub keys_available: bool,
    pub keys_size: usize,
}

#[derive(Debug)]
pub struct TargetInfo {
    pub target_url: Url,
    pub is_relay: bool,
    pub keys_available: bool,
    pub keys_size: usize,
}

/// Execute CLI command
pub async fn execute_command(cli: Cli) -> Result<()> {
    // Determine target URL and whether it's a relay
    let (target_url, is_relay, relay_gateway_url, keys_url) = match (
        cli.gateway_url.clone(),
        cli.relay_url.clone(),
        cli.relay_gateway_url.clone(),
    ) {
        // Gateway only mode
        (Some(gateway), None, None) => (gateway.clone(), false, None, gateway),
        // Invalid: relay_gateway_url without relay_url
        (Some(_), None, Some(_)) => {
            return Err(anyhow!(
                "--relay-gateway-url requires --relay-url to be specified"
            ))
        }
        // Relay only mode
        (None, Some(relay), relay_gateway) => (relay.clone(), true, relay_gateway, relay),
        // Combined mode: use relay for requests, gateway for keys
        (Some(gateway), Some(relay), relay_gateway) => (relay, true, relay_gateway, gateway),
        // No URLs provided
        (None, None, _) => {
            return Err(anyhow!(
                "At least one of --gateway-url or --relay-url must be specified"
            ))
        }
    };

    // Load OHTTP keys from file if provided
    let ohttp_keys = if let Some(key_path) = &cli.ohttp_keys {
        match std::fs::read(key_path) {
            Ok(keys) => Some(keys),
            Err(e) => {
                info!(
                    "Failed to load OHTTP keys from {}: {}, will fetch from target",
                    key_path.display(),
                    e
                );
                None
            }
        }
    } else {
        None
    };

    let client = OhttpClient::new(
        target_url,
        is_relay,
        relay_gateway_url,
        ohttp_keys,
        keys_url,
    );

    // Add any custom headers
    let headers = cli.header;

    match cli.command {
        Commands::Send {
            method,
            data,
            file,
            json,
            request_path,
        } => {
            // Validate that only one data source is provided
            let data_sources = [
                data.as_ref().map(|_| 1).unwrap_or(0),
                file.as_ref().map(|_| 1).unwrap_or(0),
                json.as_ref().map(|_| 1).unwrap_or(0),
            ];
            let data_source_count = data_sources.iter().sum::<i32>();

            if data_source_count > 1 {
                return Err(anyhow!(
                    "Only one of --data, --file, or --json can be specified"
                ));
            }

            let body_data = if let Some(data) = data {
                data.into_bytes()
            } else if let Some(file_path) = file {
                info!("Reading data from file: {}", file_path.display());
                std::fs::read(&file_path)
                    .map_err(|e| anyhow!("Failed to read file {}: {}", file_path.display(), e))?
            } else if let Some(json_data) = json {
                let mut headers = headers.clone();
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
                json_data.into_bytes()
            } else {
                Vec::new()
            };

            let target_type = if client.is_relay { "relay" } else { "gateway" };
            info!(
                "Sending {} request through {} to path: {}",
                method, target_type, request_path
            );
            info!("Request body size: {} bytes", body_data.len());

            let response = client
                .send_request(&method, &body_data, &headers, &request_path)
                .await?;
            print_response(&response)?;
        }

        Commands::GetKeys => {
            let target_type = if client.is_relay { "relay" } else { "gateway" };
            info!("Fetching OHTTP keys from {}...", target_type);
            let keys = client.fetch_keys().await?;
            info!(
                "Successfully fetched {} bytes of OHTTP key material",
                keys.len()
            );

            // Display keys as base64 for debugging
            if tracing::enabled!(tracing::Level::DEBUG) {
                let engine = base64::engine::general_purpose::STANDARD;
                debug!("Keys (base64): {}", engine.encode(&keys));
            }
        }

        Commands::Health => {
            let target_type = if client.is_relay { "relay" } else { "gateway" };
            info!("Sending health check to {}...", target_type);
            let response = client.health_check().await?;
            print_response(&response)?;
        }

        Commands::Info => {
            let target_type = if client.is_relay { "relay" } else { "gateway" };
            info!("Fetching {} information...", target_type);
            let info = client.get_target_info().await?;

            println!(
                "{} Information:",
                if info.is_relay { "Relay" } else { "Gateway" }
            );
            println!("  URL: {}", info.target_url);
            println!("  Keys available: {}", info.keys_available);
            println!("  Keys size: {} bytes", info.keys_size);

            println!();
            println!("Available endpoints:");
            if info.is_relay {
                println!(
                    "  POST {}<gateway-uri> - Forward OHTTP requests to gateway",
                    info.target_url
                );
                println!("  GET {}/health - Health check endpoint", info.target_url);
            } else {
                println!(
                    "  POST {}.well-known/ohttp-gateway - Send OHTTP requests",
                    info.target_url
                );
                println!("  GET {}/ohttp-keys - Fetch OHTTP keys", info.target_url);
                println!(
                    "  POST {}/test-gateway - Send test requests",
                    info.target_url
                );
            }
        }
    }

    Ok(())
}

/// Print response to stdout
fn print_response(response: &OhttpResponse) -> Result<()> {
    println!("HTTP/{:?} {}", response.status, response.status);
    println!("Response time: {:.2}ms", response.elapsed.as_millis());
    println!();

    // Print headers
    if !response.headers.is_empty() {
        println!("Response Headers:");
        for (name, value) in &response.headers {
            println!("  {}: {}", name, value.to_str().unwrap_or("<non-utf8>"));
        }
        println!();
    }

    // Print body
    if !response.body.is_empty() {
        println!("Response Body:");

        if response.is_json() {
            // Pretty print JSON
            if let Ok(text) = response.text() {
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&text) {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| text)
                    );
                } else {
                    println!("{}", text);
                }
            } else {
                println!("<{} bytes of binary data>", response.body.len());
            }
        } else {
            // Print as text if possible
            if let Ok(text) = response.text() {
                println!("{}", text);
            } else {
                println!("<{} bytes of binary data>", response.body.len());
            }
        }
    } else {
        println!("(empty response body)");
    }

    Ok(())
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        let gateway_url = Url::parse("http://httpbin.org").unwrap();
        let client = OhttpClient::new(gateway_url.clone(), false, None, None, gateway_url);

        let response = client.health_check().await;
        // This will fail if httpbin.org is down, but tests the structure
        match response {
            Ok(_) => {}
            Err(e) => {
                // Expected to fail if no real gateway, but should be connection error
                assert!(e.to_string().contains("connect") || e.to_string().contains("gateway"));
            }
        }
    }

    #[test]
    fn test_url_operations() {
        let base = Url::parse("http://example.com").unwrap();
        let keys_url = base.join("/ohttp-keys").unwrap();
        assert_eq!(keys_url.as_str(), "http://example.com/ohttp-keys");
    }
}
