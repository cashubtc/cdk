//! OHTTP Transport implementation
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::AuthToken;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use super::transport::Transport;
use super::Error;
use crate::error::ErrorResponse;

/// OHTTP Transport for communicating through OHTTP gateways/relays
#[derive(Clone)]
pub struct OhttpTransport {
    client: Arc<ohttp_client::OhttpClient>,
}

impl std::fmt::Debug for OhttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OhttpTransport")
            .field("client", &"Arc<OhttpClient>")
            .finish()
    }
}

impl OhttpTransport {
    /// Create new OHTTP transport with gateway and relay URLs
    ///
    /// The request flow:
    /// 1. Send to relay_url
    /// 2. Relay forwards to gateway_url
    /// 3. Gateway forwards to target_url (mint)
    /// 4. Keys are fetched from keys_source_url (same as target)
    pub fn new(target_url: Url, relay_url: Url, gateway_url: Url) -> Self {
        let client = ohttp_client::OhttpClient::new(relay_url, None, gateway_url, target_url);

        Self {
            client: Arc::new(client),
        }
    }
}

impl Default for OhttpTransport {
    fn default() -> Self {
        // Provide a minimal default that won't panic, but won't work until properly configured
        // This is needed for the Transport trait, but users should use ::new() instead
        let dummy_url = Url::parse("http://localhost").expect("Invalid default URL");
        Self::new(dummy_url.clone(), dummy_url.clone(), dummy_url)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Transport for OhttpTransport {
    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        // OHTTP transport doesn't support traditional proxies since it already
        // provides privacy through the OHTTP protocol
        Err(Error::Custom(
            "OHTTP transport does not support traditional proxies".to_string(),
        ))
    }

    async fn http_get<R>(&self, url: Url, auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        // Extract path from URL
        let path = url.path();

        // Prepare headers
        let mut headers = Vec::new();
        if let Some(auth_token) = auth {
            headers.push((auth_token.header_key().to_string(), auth_token.to_string()));
        }

        // Send GET request through OHTTP
        let response = self
            .client
            .send_ohttp_request("GET", &[], &headers, path)
            .await
            .map_err(|e| Error::Custom(format!("OHTTP request failed: {}", e)))?;

        // Check for HTTP errors
        if response.status >= 400 {
            return Err(Error::HttpError(
                Some(response.status),
                format!("HTTP {} error", response.status),
            ));
        }

        // Parse response body
        let response_text = response
            .text()
            .map_err(|e| Error::Custom(format!("Failed to decode response: {}", e)))?;

        serde_json::from_str::<R>(&response_text).map_err(|err| {
            tracing::warn!("OHTTP Response error: {}", err);
            match ErrorResponse::from_json(&response_text) {
                Ok(error_response) => error_response.into(),
                Err(parse_err) => parse_err.into(),
            }
        })
    }

    async fn http_post<P, R>(
        &self,
        url: Url,
        auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned,
    {
        // Extract path from URL
        let path = url.path();

        // Serialize payload to JSON
        let body = serde_json::to_vec(payload)
            .map_err(|e| Error::Custom(format!("Failed to serialize payload: {}", e)))?;

        // Prepare headers
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if let Some(auth) = auth_token {
            headers.push((auth.header_key().to_string(), auth.to_string()));
        }

        // Send POST request through OHTTP
        let response = self
            .client
            .send_ohttp_request("POST", &body, &headers, path)
            .await
            .map_err(|e| Error::Custom(format!("OHTTP request failed: {}", e)))?;

        // Check for HTTP errors
        if response.status >= 400 {
            return Err(Error::HttpError(
                Some(response.status),
                format!("HTTP {} error", response.status),
            ));
        }

        // Parse response body
        let response_text = response
            .text()
            .map_err(|e| Error::Custom(format!("Failed to decode response: {}", e)))?;

        serde_json::from_str::<R>(&response_text).map_err(|err| {
            tracing::warn!("OHTTP Response error: {}", err);
            match ErrorResponse::from_json(&response_text) {
                Ok(error_response) => error_response.into(),
                Err(parse_err) => parse_err.into(),
            }
        })
    }
}
