//! Authentication providers for NpubCash API
//!
//! Implements NIP-98 and JWT authentication

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};
use tokio::sync::RwLock;
use web_time::SystemTime;

use crate::npubcash::types::Nip98Response;
use crate::npubcash::{Error, Result};

struct CachedToken {
    token: String,
    expires_at: SystemTime,
}

impl fmt::Debug for CachedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedToken")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// JWT authentication provider using NIP-98
pub struct JwtAuthProvider {
    base_url: String,
    keys: Keys,
    http_client: cdk_common::HttpClient,
    cached_token: Arc<RwLock<Option<CachedToken>>>,
}

impl fmt::Debug for JwtAuthProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtAuthProvider")
            .field("base_url", &self.base_url)
            .field("keys", &"[REDACTED]")
            .field("http_client", &self.http_client)
            .field("cached_token", &self.cached_token)
            .finish()
    }
}

impl JwtAuthProvider {
    /// Create a new JWT authentication provider
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the NpubCash service
    /// * `keys` - Nostr keys for signing NIP-98 tokens
    pub fn new(base_url: String, keys: Keys) -> Self {
        Self {
            base_url,
            keys,
            http_client: cdk_common::HttpClient::new(),
            cached_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Ensure we have a valid cached JWT token, fetching a new one if needed
    ///
    /// This method checks the cache first and returns the cached token if it's still valid.
    /// If the cache is empty or expired, it fetches a new JWT token from the API.
    ///
    /// # Errors
    ///
    /// Returns an error if token generation or API request fails
    async fn ensure_cached_token(&self) -> Result<String> {
        // Check if we have a valid cached token
        if let Some(token) = self.get_valid_cached_token().await {
            return Ok(token);
        }

        // Fetch a new JWT token from the API
        let token = self.fetch_fresh_jwt_token().await?;

        // Cache the new token
        self.cache_token(&token).await;

        Ok(token)
    }

    /// Get a valid token from cache, if one exists and hasn't expired
    async fn get_valid_cached_token(&self) -> Option<String> {
        let cache = self.cached_token.read().await;
        cache.as_ref().and_then(|cached| {
            if cached.expires_at > SystemTime::now() {
                Some(cached.token.clone())
            } else {
                None
            }
        })
    }

    /// Fetch a fresh JWT token from the NpubCash API using NIP-98 authentication
    async fn fetch_fresh_jwt_token(&self) -> Result<String> {
        let auth_url = format!("{}/api/v2/auth/nip98", self.base_url);

        // Create NIP-98 token for authentication
        let nostr_token = self.create_nip98_token_for_auth(&auth_url)?;

        // Send authentication request
        let response = self.send_auth_request(&auth_url, &nostr_token).await?;

        // Parse and validate response
        self.parse_jwt_response(response).await
    }

    /// Create a NIP-98 token for authentication
    fn create_nip98_token_for_auth(&self, auth_url: &str) -> Result<String> {
        tracing::debug!("Creating NIP-98 token for URL: {}", auth_url);
        self.create_nip98_token(auth_url, "GET")
    }

    /// Send the authentication request to the API
    async fn send_auth_request(
        &self,
        auth_url: &str,
        nostr_token: &str,
    ) -> Result<cdk_common::RawResponse> {
        tracing::debug!("Sending request to: {}", auth_url);

        let response = self
            .http_client
            .get(auth_url)
            .header("Authorization", format!("Nostr {nostr_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                concat!("cdk-nostr/", env!("CARGO_PKG_VERSION")),
            )
            .send()
            .await?;

        tracing::debug!("Response status: {}", response.status());
        Ok(response)
    }

    /// Parse the JWT response from the API
    async fn parse_jwt_response(&self, response: cdk_common::RawResponse) -> Result<String> {
        let status = response.status();

        if !response.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            tracing::error!("Auth failed - Status: {}, Body: {}", status, error_text);
            return Err(Error::Auth(format!(
                "Failed to get JWT: {status} - {error_text}"
            )));
        }

        let nip98_response: Nip98Response = response.json().await?;
        Ok(nip98_response.data.token)
    }

    /// Cache the JWT token with a 5-minute expiration
    async fn cache_token(&self, token: &str) {
        let expires_at = SystemTime::now() + Duration::from_secs(5 * 60);
        let mut cache = self.cached_token.write().await;
        *cache = Some(CachedToken {
            token: token.to_string(),
            expires_at,
        });
    }

    fn create_nip98_token(&self, url: &str, method: &str) -> Result<String> {
        let u_tag = Tag::custom("u", [url]);
        let method_tag = Tag::custom("method", [method]);

        let event = EventBuilder::new(Kind::HttpAuth, "")
            .tags(vec![u_tag, method_tag])
            .finalize(&self.keys)
            .map_err(|e| Error::Nostr(e.to_string()))?;

        let json = serde_json::to_string(&event)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        tracing::debug!("Base64 encoded token length: {}", encoded.len());
        Ok(encoded)
    }

    /// Get a Bearer token for authenticated requests
    ///
    /// # Arguments
    ///
    /// * `_url` - The URL being accessed (unused, kept for future extensibility)
    /// * `_method` - The HTTP method being used (unused, kept for future extensibility)
    ///
    /// # Errors
    ///
    /// Returns an error if token generation or fetching fails
    pub async fn get_auth_token(&self, _url: &str, _method: &str) -> Result<String> {
        let token = self.ensure_cached_token().await?;
        Ok(format!("Bearer {token}"))
    }

    /// Get a NIP-98 auth header for direct authentication
    ///
    /// This creates a fresh NIP-98 signed event for the specific URL and method,
    /// returning the full Authorization header value (e.g., "Nostr <base64_event>").
    ///
    /// # Arguments
    ///
    /// * `url` - The URL being accessed
    /// * `method` - The HTTP method being used (GET, POST, PATCH, etc.)
    ///
    /// # Errors
    ///
    /// Returns an error if token generation fails
    pub fn get_nip98_auth_header(&self, url: &str, method: &str) -> Result<String> {
        let token = self.create_nip98_token(url, method)?;
        Ok(format!("Nostr {token}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_token_debug_redacts_token() {
        let secret = "replayable-jwt-token";
        let token = CachedToken {
            token: secret.to_string(),
            expires_at: SystemTime::now(),
        };

        let debug = format!("{token:?}");

        assert!(!debug.contains(secret));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn auth_provider_debug_redacts_signing_key() {
        let keys = Keys::generate();
        let secret = keys.secret_key().to_secret_hex();
        let provider = JwtAuthProvider::new("https://npub.cash".to_string(), keys);

        let debug = format!("{provider:?}");

        assert!(!debug.contains(&secret));
        assert!(debug.contains("keys: \"[REDACTED]\""));
    }
}
