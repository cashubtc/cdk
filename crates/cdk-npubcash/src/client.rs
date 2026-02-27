//! HTTP client for NpubCash API

use std::sync::Arc;

use cdk_http_client::{HttpClient, RawResponse, RequestBuilderExt};
use tracing::instrument;

use crate::auth::JwtAuthProvider;
use crate::error::{Error, Result};
use crate::types::{Quote, QuotesResponse};

const API_PATHS_QUOTES: &str = "/api/v2/wallet/quotes";
const PAGINATION_LIMIT: usize = 50;
const THROTTLE_DELAY_MS: u64 = 200;

/// Main client for interacting with the NpubCash API
pub struct NpubCashClient {
    base_url: String,
    auth_provider: Arc<JwtAuthProvider>,
    http_client: HttpClient,
}

impl std::fmt::Debug for NpubCashClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NpubCashClient")
            .field("base_url", &self.base_url)
            .field("auth_provider", &self.auth_provider)
            .finish_non_exhaustive()
    }
}

impl NpubCashClient {
    /// Create a new NpubCash client
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the NpubCash service (e.g., <https://npubx.cash>)
    /// * `auth_provider` - Authentication provider for signing requests
    pub fn new(base_url: String, auth_provider: Arc<JwtAuthProvider>) -> Self {
        Self {
            base_url,
            auth_provider,
            http_client: HttpClient::new(),
        }
    }

    /// Fetch quotes, optionally filtered by timestamp
    ///
    /// # Arguments
    ///
    /// * `since` - Optional Unix timestamp to fetch quotes from. If `None`, fetches all quotes.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
    /// # use nostr_sdk::Keys;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let base_url = "https://npubx.cash".to_string();
    /// # let keys = Keys::generate();
    /// # let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
    /// # let client = NpubCashClient::new(base_url, auth_provider);
    /// // Fetch all quotes
    /// let all_quotes = client.get_quotes(None).await?;
    ///
    /// // Fetch quotes since a specific timestamp
    /// let recent_quotes = client.get_quotes(Some(1234567890)).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self))]
    pub async fn get_quotes(&self, since: Option<u64>) -> Result<Vec<Quote>> {
        if let Some(ts) = since {
            tracing::debug!("Fetching quotes since timestamp: {}", ts);
        } else {
            tracing::debug!("Fetching all quotes");
        }
        self.fetch_paginated_quotes(since).await
    }

    /// Fetch quotes with pagination support
    ///
    /// This method handles automatic pagination, fetching all available quotes
    /// matching the criteria. It throttles requests to avoid overwhelming the API.
    ///
    /// # Arguments
    ///
    /// * `since` - Optional timestamp to filter quotes created after this time
    ///
    /// # Errors
    ///
    /// Returns an error if any page fetch fails
    async fn fetch_paginated_quotes(&self, since: Option<u64>) -> Result<Vec<Quote>> {
        let mut all_quotes = Vec::new();
        let mut offset = 0;

        loop {
            // Build the URL for this page
            let url = self.build_quotes_url(offset, since)?;

            // Fetch the current page
            let response: QuotesResponse = self.authenticated_request(url.as_str(), "GET").await?;

            // Collect quotes from this page
            let fetched_count = response.data.quotes.len();
            all_quotes.extend(response.data.quotes);

            tracing::debug!(
                "Fetched {} quotes. Total fetched: {}",
                fetched_count,
                all_quotes.len()
            );

            // Check if we should continue paginating
            offset += PAGINATION_LIMIT;
            if !Self::should_fetch_next_page(offset, response.metadata.total) {
                break;
            }

            // Throttle to avoid overwhelming the API
            self.throttle_request().await;
        }

        tracing::info!(
            "Successfully fetched a total of {} quotes",
            all_quotes.len()
        );
        Ok(all_quotes)
    }

    /// Build the URL for fetching quotes with pagination and filters
    fn build_quotes_url(&self, offset: usize, since: Option<u64>) -> Result<url::Url> {
        let mut url = url::Url::parse(&format!("{}{}", self.base_url, API_PATHS_QUOTES))?;

        // Add pagination parameters
        url.query_pairs_mut()
            .append_pair("offset", &offset.to_string())
            .append_pair("limit", &PAGINATION_LIMIT.to_string());

        // Add optional timestamp filter
        if let Some(since_val) = since {
            url.query_pairs_mut()
                .append_pair("since", &since_val.to_string());
        }

        Ok(url)
    }

    /// Set the mint URL for the user
    ///
    /// Updates the default mint URL used by the NpubCash server when creating quotes.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - URL of the Cashu mint to use
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails.
    /// Returns `UnsupportedEndpoint` if the server doesn't support this feature.
    #[instrument(skip(self, mint_url))]
    pub async fn set_mint_url(
        &self,
        mint_url: impl Into<String>,
    ) -> Result<crate::types::UserResponse> {
        use serde::Serialize;

        const MINT_URL_PATH: &str = "/api/v2/user/mint";

        #[derive(Serialize)]
        struct MintUrlPayload {
            mint_url: String,
        }

        let url = format!("{}{}", self.base_url, MINT_URL_PATH);
        let payload = MintUrlPayload {
            mint_url: mint_url.into(),
        };

        // Get NIP-98 authentication header (not JWT Bearer)
        let auth_header = self.auth_provider.get_nip98_auth_header(&url, "PATCH")?;

        // Send PATCH request
        let response = self
            .http_client
            .patch(&url)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "cdk-npubcash/0.13.0")
            .json(&payload)
            .send()
            .await?;

        let status = response.status();

        // Handle error responses
        if !response.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: error_text,
                status,
            });
        }

        // Get response text for debugging
        let response_text = response.text().await?;
        tracing::debug!("set_mint_url response: {}", response_text);

        // Parse JSON response
        serde_json::from_str(&response_text).map_err(|e| {
            tracing::error!("Failed to parse response: {} - Body: {}", e, response_text);
            Error::Custom(format!("JSON parse error: {e}"))
        })
    }

    /// Determine if we should fetch the next page of results
    const fn should_fetch_next_page(current_offset: usize, total_available: usize) -> bool {
        current_offset < total_available
    }

    /// Throttle requests to avoid overwhelming the API
    async fn throttle_request(&self) {
        tracing::debug!("Throttling for {}ms...", THROTTLE_DELAY_MS);
        tokio::time::sleep(tokio::time::Duration::from_millis(THROTTLE_DELAY_MS)).await;
    }

    /// Make an authenticated HTTP request to the API
    ///
    /// This method handles authentication, sends the request, and parses the response.
    ///
    /// # Arguments
    ///
    /// * `url` - Full URL to request
    /// * `method` - HTTP method (e.g., "GET", "POST")
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails, request fails, or response parsing fails
    async fn authenticated_request<T>(&self, url: &str, method: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        // Extract URL for authentication (without query parameters)
        let url_for_auth = crate::extract_auth_url(url)?;

        // Get authentication token
        let auth_token = self
            .auth_provider
            .get_auth_token(&url_for_auth, method)
            .await?;

        // Send the HTTP request with authentication headers
        tracing::debug!("Making {} request to {}", method, url);
        let response = self
            .http_client
            .get(url)
            .header("Authorization", auth_token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "cdk-npubcash/0.13.0")
            .send()
            .await?;

        tracing::debug!("Response status: {}", response.status());

        // Parse and return the JSON response
        self.parse_response(response).await
    }

    /// Parse the HTTP response and deserialize the JSON body
    async fn parse_response<T>(&self, response: RawResponse) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();

        // Get the response text
        let response_text = response.text().await?;

        // Handle error status codes
        if !(200..300).contains(&status) {
            tracing::debug!("Error response ({}): {}", status, response_text);
            return Err(Error::Api {
                message: response_text,
                status,
            });
        }

        // Parse successful JSON response
        tracing::debug!("Response body: {}", response_text);
        let data = serde_json::from_str::<T>(&response_text).map_err(|e| {
            tracing::error!("JSON parse error: {} - Body: {}", e, response_text);
            Error::Custom(format!("JSON parse error: {e}"))
        })?;

        tracing::debug!("Request successful");
        Ok(data)
    }
}
