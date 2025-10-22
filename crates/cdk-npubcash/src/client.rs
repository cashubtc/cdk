//! HTTP client for NpubCash API

use std::sync::Arc;

use reqwest::Client as HttpClient;
use tracing::instrument;

use crate::auth::AuthProvider;
use crate::error::{Error, Result};
use crate::settings::SettingsManager;
use crate::types::{Quote, QuotesResponse};

const API_PATHS_QUOTES: &str = "/api/v2/wallet/quotes";
const PAGINATION_LIMIT: usize = 50;
const THROTTLE_DELAY_MS: u64 = 200;

/// Main client for interacting with the NpubCash API
pub struct NpubCashClient {
    base_url: String,
    auth_provider: Arc<dyn AuthProvider>,
    http_client: HttpClient,
    /// Settings manager for user preferences
    pub settings: SettingsManager,
}

impl std::fmt::Debug for NpubCashClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NpubCashClient")
            .field("base_url", &self.base_url)
            .field("auth_provider", &self.auth_provider)
            .field("settings", &self.settings)
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
    pub fn new(base_url: String, auth_provider: Arc<dyn AuthProvider>) -> Self {
        let settings = SettingsManager::new(base_url.clone(), Arc::clone(&auth_provider));
        Self {
            base_url,
            auth_provider,
            http_client: HttpClient::new(),
            settings,
        }
    }

    /// Fetch quotes since a specific timestamp
    ///
    /// # Arguments
    ///
    /// * `since` - Unix timestamp to fetch quotes from
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    #[instrument(skip(self))]
    pub async fn get_quotes_since(&self, since: u64) -> Result<Vec<Quote>> {
        tracing::debug!("Fetching quotes since timestamp: {}", since);
        self.fetch_paginated_quotes(Some(since)).await
    }

    /// Fetch all quotes
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    #[instrument(skip(self))]
    pub async fn get_all_quotes(&self) -> Result<Vec<Quote>> {
        tracing::debug!("Fetching all quotes");
        self.fetch_paginated_quotes(None).await
    }

    /// Poll for new quotes with a callback function
    ///
    /// This method continuously polls for new quotes at the specified interval
    /// and calls the callback function when new quotes are found.
    ///
    /// # Arguments
    ///
    /// * `interval` - Duration between polls
    /// * `on_update` - Callback function called with new quotes
    ///
    /// # Errors
    ///
    /// Returns an error if the initial poll fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
    /// # use nostr_sdk::Keys;
    /// # use std::sync::Arc;
    /// # use std::time::Duration;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let base_url = "https://npubx.cash".to_string();
    /// # let keys = Keys::generate();
    /// # let auth_provider = JwtAuthProvider::new(base_url.clone(), keys);
    /// # let client = NpubCashClient::new(base_url, Arc::new(auth_provider));
    /// let handle = client
    ///     .poll_quotes_with_callback(Duration::from_secs(10), |quotes| {
    ///         println!("Found {} new quotes", quotes.len());
    ///     })
    ///     .await?;
    ///
    /// // Keep the handle alive to continue polling
    /// // When the handle is dropped, polling will stop
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, on_update))]
    pub async fn poll_quotes_with_callback<F>(
        &self,
        interval: std::time::Duration,
        mut on_update: F,
    ) -> Result<PollingHandle>
    where
        F: FnMut(Vec<Quote>) + Send + 'static,
    {
        use tokio::sync::mpsc;
        use tokio::time::sleep;

        // Get initial timestamp
        let mut last_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Custom(e.to_string()))?
            .as_secs();

        let (tx, mut rx) = mpsc::channel::<()>(1);
        let base_url = self.base_url.clone();
        let auth_provider = Arc::clone(&self.auth_provider);

        tokio::spawn(async move {
            let client = Self::new(base_url, auth_provider);

            loop {
                tokio::select! {
                    () = sleep(interval) => {
                        match client.get_quotes_since(last_timestamp).await {
                            Ok(quotes) => {
                                if !quotes.is_empty() {
                                    tracing::debug!("Found {} new quotes", quotes.len());
                                    // Update timestamp to most recent quote
                                    if let Some(max_ts) = quotes.iter().map(|q| q.created_at).max() {
                                        last_timestamp = max_ts;
                                    }
                                    on_update(quotes);
                                }
                            }
                            Err(e) => {
                                tracing::debug!("Error polling quotes: {}", e);
                            }
                        }
                    }
                    Some(()) = rx.recv() => {
                        tracing::debug!("Polling cancelled");
                        break;
                    }
                }
            }
        });

        Ok(PollingHandle { _cancel: tx })
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
        let url_for_auth = Self::extract_auth_url(url)?;

        // Get authentication token
        let auth_token = self
            .auth_provider
            .get_auth_token(&url_for_auth, method)
            .await?;

        // Send the HTTP request
        let response = self.send_http_request(url, method, &auth_token).await?;

        // Parse and return the JSON response
        self.parse_response(response).await
    }

    /// Extract the URL components needed for authentication (scheme + host + path)
    ///
    /// Query parameters are excluded from the authentication URL.
    fn extract_auth_url(url: &str) -> Result<String> {
        let parsed_url = url::Url::parse(url)?;
        let host = parsed_url
            .host_str()
            .ok_or_else(|| Error::Custom("Invalid URL: missing host".to_string()))?;

        Ok(format!(
            "{}://{}{}",
            parsed_url.scheme(),
            host,
            parsed_url.path()
        ))
    }

    /// Send an HTTP request with authentication headers
    async fn send_http_request(
        &self,
        url: &str,
        method: &str,
        auth_token: &str,
    ) -> Result<reqwest::Response> {
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
        Ok(response)
    }

    /// Parse the HTTP response and deserialize the JSON body
    async fn parse_response<T>(&self, response: reqwest::Response) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let status = response.status();

        // Check for error status codes
        if !status.is_success() {
            return self.handle_error_response(response, status).await;
        }

        // Parse successful response
        self.parse_json_body(response).await
    }

    /// Handle an error response from the API
    async fn handle_error_response<T>(
        &self,
        response: reqwest::Response,
        status: reqwest::StatusCode,
    ) -> Result<T> {
        let error_text = response.text().await.unwrap_or_default();
        tracing::debug!("Error response body: {}", error_text);

        Err(Error::Api {
            message: error_text,
            status: status.as_u16(),
        })
    }

    /// Parse and deserialize the JSON response body
    async fn parse_json_body<T>(&self, response: reqwest::Response) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        // Get the raw response text for debugging
        let response_text = response.text().await?;
        tracing::debug!(
            "Response body (first 500 chars): {}",
            &response_text[..response_text.len().min(500)]
        );

        // Try to parse the JSON
        let data = serde_json::from_str::<T>(&response_text).map_err(|e| {
            tracing::error!("Failed to parse JSON response: {}", e);
            tracing::error!("Full response body: {}", response_text);
            Error::Custom(format!("JSON parse error: {e}"))
        })?;

        tracing::debug!("Request successful");
        Ok(data)
    }
}

/// Handle for managing an active polling task
///
/// When this handle is dropped, the polling task will be cancelled.
#[derive(Debug)]
pub struct PollingHandle {
    _cancel: tokio::sync::mpsc::Sender<()>,
}

impl Drop for PollingHandle {
    fn drop(&mut self) {
        tracing::debug!("Dropping polling handle");
    }
}
