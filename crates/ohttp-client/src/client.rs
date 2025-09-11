use std::sync::Arc;

use anyhow::{anyhow, Result};
use http::HeaderMap;
use reqwest::Client;
use tokio::sync::RwLock;
use url::Url;

/// OHTTP client for sending requests through gateways or relays
pub struct OhttpClient {
    client: Client,
    relay_url: Url,
    ohttp_keys: Arc<RwLock<Option<Vec<u8>>>>,
    gateway_url: Url,
    target_url: Url,
}

impl OhttpClient {
    /// Create a new OHTTP client
    pub fn new(
        relay_url: Url,
        ohttp_keys: Option<Vec<u8>>,
        gateway_url: Url,
        target_url: Url,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            relay_url,
            ohttp_keys: Arc::new(RwLock::new(ohttp_keys)),
            gateway_url,
            target_url,
        }
    }

    /// Fetch OHTTP keys from the keys source (can be different from target URL)
    pub async fn fetch_keys(&self) -> Result<Vec<u8>> {
        let keys_url = self.gateway_url.join("/ohttp-keys")?;

        tracing::debug!("Fetching OHTTP keys from: {}", keys_url);

        let response = self.client.get(keys_url).send().await?.error_for_status()?;

        let keys = response.bytes().await?;
        tracing::debug!("Fetched OHTTP keys, size: {} bytes", keys.len());

        let mut ohttp_keys = self.ohttp_keys.write().await;

        *ohttp_keys = Some(keys.to_vec());

        Ok(keys.to_vec())
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
        let maybe_keys = {
            let guard = self.ohttp_keys.read().await;
            guard.clone()
        };

        let keys_data = match maybe_keys {
            Some(keys) => keys,
            None => self.fetch_keys().await?,
        };

        // Parse the OHTTP keys and create client request
        let client_request = ohttp::ClientRequest::from_encoded_config(&keys_data)
            .map_err(|e| anyhow!("Failed to decode OHTTP keys: {}", e))?;

        tracing::debug!("Created OHTTP client request");

        // Create BHTTP request
        let bhttp_request = self.create_bhttp_request(method, body, headers, request_path)?;
        tracing::debug!("Created BHTTP request, size: {} bytes", bhttp_request.len());

        // Encapsulate the request using OHTTP
        let (ohttp_request, response_context) = client_request
            .encapsulate(&bhttp_request)
            .map_err(|e| anyhow!("Failed to encapsulate OHTTP request: {}", e))?;

        tracing::debug!(
            "Encapsulated OHTTP request, size: {} bytes",
            ohttp_request.len()
        );

        // Send directly to the target URL without appending .well-known/ohttp-gateway
        let endpoint_url = self.relay_url.clone();

        tracing::debug!("Sending OHTTP request to: {}", endpoint_url);

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

        tracing::debug!(
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
            tracing::debug!("Warning: Unexpected content type: {}", content_type);
        }

        let _response_status = response.status().as_u16();
        let _response_headers = response.headers().clone();
        let ohttp_response_body = response.bytes().await?;

        tracing::debug!(
            "OHTTP response body size: {} bytes",
            ohttp_response_body.len()
        );

        // Decapsulate the OHTTP response
        let bhttp_response = response_context
            .decapsulate(&ohttp_response_body)
            .map_err(|e| anyhow!("Failed to decapsulate OHTTP response: {}", e))?;

        tracing::debug!(
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

        tracing::debug!("Creating BHTTP request: {} {}", method, request_path);

        // Extract proper authority from target URL (host:port only, no scheme)
        let authority = if let Some(port) = self.target_url.port() {
            format!(
                "{}:{}",
                self.target_url.host_str().unwrap_or("localhost"),
                port
            )
        } else {
            self.target_url
                .host_str()
                .unwrap_or("localhost")
                .to_string()
        };

        tracing::debug!(
            "Using authority: {} for target: {}",
            authority,
            self.target_url
        );

        // Create the BHTTP message
        let mut bhttp_msg = Message::request(
            method.as_bytes().to_vec(),
            self.target_url.scheme().as_bytes().to_vec(), // scheme from target URL
            authority.as_bytes().to_vec(),                // authority (host:port only)
            request_path.as_bytes().to_vec(),             // path
        );

        // Add headers
        for (name, value) in headers {
            bhttp_msg.put_header(name.as_bytes(), value.as_bytes());
            tracing::debug!("Added header: {}: {}", name, value);
        }

        // Add body
        if !body.is_empty() {
            bhttp_msg.write_content(body);
            tracing::debug!("Added body, size: {} bytes", body.len());
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

        tracing::debug!("Parsing BHTTP response, size: {} bytes", bhttp_bytes.len());

        let mut cursor = std::io::Cursor::new(bhttp_bytes);
        let bhttp_msg = Message::read_bhttp(&mut cursor)
            .map_err(|e| anyhow!("Failed to read BHTTP response: {}", e))?;

        // Extract status
        let status = bhttp_msg
            .control()
            .status()
            .ok_or_else(|| anyhow!("Missing status in BHTTP response"))?;

        tracing::debug!("Parsed status: {}", u16::from(status));

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
                tracing::debug!("Parsed header: {}: {}", name, value);
            }
        }

        // Extract body
        let body = bhttp_msg.content().to_vec();
        tracing::debug!("Parsed body, size: {} bytes", body.len());

        Ok((status.into(), headers, body))
    }

    /// Get target information
    pub async fn get_target_info(&self) -> Result<TargetInfo> {
        let keys = self.fetch_keys().await?;

        Ok(TargetInfo {
            target_url: self.relay_url.clone(),
            keys_available: true,
            keys_size: keys.len(),
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
    pub keys_available: bool,
    pub keys_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authority_extraction() {
        // Test with port
        let target_url = Url::parse("http://127.0.0.1:8085").unwrap();
        let _client = OhttpClient::new(
            target_url.clone(),
            None,
            target_url.clone(),
            target_url.clone(),
        );

        let authority = if let Some(port) = target_url.port() {
            format!("{}:{}", target_url.host_str().unwrap_or("localhost"), port)
        } else {
            target_url.host_str().unwrap_or("localhost").to_string()
        };

        assert_eq!(authority, "127.0.0.1:8085");

        // Test without explicit port (default ports)
        let target_url_no_port = Url::parse("https://example.com").unwrap();
        let authority_no_port = if let Some(port) = target_url_no_port.port() {
            format!(
                "{}:{}",
                target_url_no_port.host_str().unwrap_or("localhost"),
                port
            )
        } else {
            target_url_no_port
                .host_str()
                .unwrap_or("localhost")
                .to_string()
        };

        assert_eq!(authority_no_port, "example.com");
    }

    #[test]
    fn test_authority_does_not_include_scheme() {
        let target_url = Url::parse("https://example.com:8443/some/path").unwrap();

        let authority = if let Some(port) = target_url.port() {
            format!("{}:{}", target_url.host_str().unwrap_or("localhost"), port)
        } else {
            target_url.host_str().unwrap_or("localhost").to_string()
        };

        // Authority should NOT include scheme or path
        assert_eq!(authority, "example.com:8443");
        assert!(!authority.contains("https://"));
        assert!(!authority.contains("/some/path"));
    }
}
