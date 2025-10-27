//! Authentication providers for NpubCash API
//!
//! Implements NIP-98 and JWT authentication

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use nostr_sdk::{EventBuilder, Keys, Kind, Tag};
use tokio::sync::RwLock;

use crate::types::Nip98Response;
use crate::{Error, Result};

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: SystemTime,
}

/// JWT authentication provider using NIP-98
#[derive(Debug)]
pub struct JwtAuthProvider {
    base_url: String,
    keys: Keys,
    http_client: reqwest::Client,
    cached_token: Arc<RwLock<Option<CachedToken>>>,
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
            http_client: reqwest::Client::new(),
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
        let nostr_token = self.create_nip98_token_with_logging(&auth_url)?;

        // Send authentication request
        let response = self.send_auth_request(&auth_url, &nostr_token).await?;

        // Parse and validate response
        self.parse_jwt_response(response).await
    }

    /// Create a NIP-98 token with debug logging
    fn create_nip98_token_with_logging(&self, auth_url: &str) -> Result<String> {
        tracing::debug!("Creating NIP-98 token for URL: {}", auth_url);
        let nostr_token = self.create_nip98_token(auth_url, "GET")?;
        tracing::debug!(
            "NIP-98 token created (first 50 chars): {}",
            &nostr_token[..50.min(nostr_token.len())]
        );
        Ok(nostr_token)
    }

    /// Send the authentication request to the API
    async fn send_auth_request(
        &self,
        auth_url: &str,
        nostr_token: &str,
    ) -> Result<reqwest::Response> {
        tracing::debug!("Sending request to: {}", auth_url);
        tracing::debug!(
            "Authorization header: Nostr {}",
            &nostr_token[..50.min(nostr_token.len())]
        );

        let response = self
            .http_client
            .get(auth_url)
            .header("Authorization", format!("Nostr {nostr_token}"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "cdk-npubcash/0.13.0")
            .send()
            .await?;

        tracing::debug!("Response status: {}", response.status());
        Ok(response)
    }

    /// Parse the JWT response from the API
    async fn parse_jwt_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
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
        let u_tag = Tag::custom(
            nostr_sdk::TagKind::Custom(std::borrow::Cow::Borrowed("u")),
            vec![url],
        );
        let method_tag = Tag::custom(
            nostr_sdk::TagKind::Custom(std::borrow::Cow::Borrowed("method")),
            vec![method],
        );

        let event = EventBuilder::new(Kind::Custom(27235), "")
            .tags(vec![u_tag, method_tag])
            .sign_with_keys(&self.keys)
            .map_err(|e| Error::Nostr(e.to_string()))?;

        let json = serde_json::to_string(&event)?;
        tracing::debug!("NIP-98 event JSON: {}", json);
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
}
