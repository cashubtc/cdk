//! HTTP client for NpubCash API

use std::sync::Arc;

use cdk_http_client::{HttpClient, RawResponse};
use tracing::instrument;

use crate::npubcash::auth::JwtAuthProvider;
use crate::npubcash::error::{Error, Result};
use crate::npubcash::types::{MissingQuotesRequest, Quote, QuotesData, QuotesResponse};

const API_PATHS_QUOTES: &str = "/api/v2/wallet/quotes";
const API_PATHS_QUOTES_MISSING: &str = "/api/v2/wallet/quotes/missing";
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
    /// # use cdk_nostr::npubcash::{NpubCashClient, JwtAuthProvider};
    /// # use cdk_nostr::nostr_sdk::prelude::Keys;
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

    /// Resolve full quote data for specific quote IDs
    ///
    /// Asks the NpubCash server for the quotes matching `quote_ids`. This is
    /// used to reconcile the local wallet with the server: fetch all quote
    /// IDs, determine which ones are unknown locally, and resolve only those.
    ///
    /// # Arguments
    ///
    /// * `quote_ids` - Quote IDs to resolve
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails.
    /// Returns an API error with status 404 if the server does not support
    /// this endpoint yet.
    #[instrument(skip(self, quote_ids))]
    pub async fn get_missing_quotes(&self, quote_ids: &[String]) -> Result<Vec<Quote>> {
        if quote_ids.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{}{}", self.base_url, API_PATHS_QUOTES_MISSING);
        let payload = MissingQuotesRequest {
            quote_ids: quote_ids.to_vec(),
        };

        let auth_header = self.auth_provider.get_nip98_auth_header(&url, "POST")?;

        tracing::debug!("Resolving {} quote IDs", quote_ids.len());
        let response = self
            .http_client
            .post(&url)
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                concat!("cdk-nostr/", env!("CARGO_PKG_VERSION")),
            )
            .json(&payload)
            .send()
            .await?;

        let data: QuotesData = self.parse_response(response).await?;
        Ok(data.quotes)
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
            let response: QuotesResponse = self.authenticated_get(url.as_str()).await?;

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
    ) -> Result<crate::npubcash::types::UserResponse> {
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
            .header(
                "User-Agent",
                concat!("cdk-nostr/", env!("CARGO_PKG_VERSION")),
            )
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

    /// Enable or disable NUT-20 quote locking for the user's NpubCash account
    ///
    /// When enabled, the NpubCash server creates new mint quotes locked to the
    /// user's Nostr public key, so minting them requires a NUT-20 quote
    /// signature from the matching secret key. The server rejects enabling
    /// locking when the configured mint does not support NUT-20.
    ///
    /// Already-created quotes keep their original lock state.
    ///
    /// Two server layouts exist in the wild: npubx-style servers (and
    /// npub.cash production) expose `PATCH /api/v2/user/lock`, while the
    /// npub.cash API docs describe `PUT /api/v2/settings/lock`. The live-server
    /// endpoint is tried first, falling back to the documented one when it is
    /// not implemented.
    ///
    /// # Arguments
    ///
    /// * `lock_quotes` - Whether new quotes should be locked to the user's npub
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails.
    #[instrument(skip(self))]
    pub async fn set_quote_locking(
        &self,
        lock_quotes: bool,
    ) -> Result<crate::npubcash::types::UserResponse> {
        use serde::Serialize;

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct LockPayload {
            lock_quotes: bool,
        }

        let payload = LockPayload { lock_quotes };

        match self
            .send_settings_request("PATCH", "/api/v2/user/lock", Some(&payload))
            .await
        {
            Ok(response) => Ok(response),
            Err(Error::Api { status: 404, .. }) => {
                tracing::debug!(
                    "Server does not support PATCH /api/v2/user/lock; trying PUT /api/v2/settings/lock"
                );
                self.send_settings_request("PUT", "/api/v2/settings/lock", Some(&payload))
                    .await
            }
            Err(error) => Err(error),
        }
    }

    /// Fetch the user's NpubCash settings
    ///
    /// Returns the user's configured mint URL and whether quote locking is
    /// enabled for their account.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails.
    #[instrument(skip(self))]
    pub async fn get_user_info(&self) -> Result<crate::npubcash::types::UserResponse> {
        match self
            .send_settings_request::<&'static ()>("GET", "/api/v2/user/info", None)
            .await
        {
            Ok(response) => Ok(response),
            Err(Error::Api { status: 404, .. }) => {
                tracing::debug!(
                    "Server does not support GET /api/v2/user/info; trying GET /api/v2/settings"
                );
                self.send_settings_request::<&'static ()>("GET", "/api/v2/settings", None)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    /// Send a settings request with NIP-98 authentication and parse the
    /// user-settings response.
    async fn send_settings_request<T: serde::Serialize>(
        &self,
        method: &str,
        path: &str,
        payload: Option<&T>,
    ) -> Result<crate::npubcash::types::UserResponse> {
        let url = format!("{}{}", self.base_url, path);
        let auth_header = self.auth_provider.get_nip98_auth_header(&url, method)?;

        let builder = match method {
            "GET" => self.http_client.get(&url),
            "PATCH" => self.http_client.patch(&url),
            "PUT" => self.http_client.put(&url),
            other => {
                return Err(Error::Custom(format!(
                    "Unsupported settings method: {other}"
                )))
            }
        };

        let builder = builder
            .header("Authorization", auth_header)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                concat!("cdk-nostr/", env!("CARGO_PKG_VERSION")),
            );

        let builder = match payload {
            Some(payload) => builder.json(payload),
            None => builder,
        };

        let response = builder.send().await?;

        let status = response.status();

        if !response.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: error_text,
                status,
            });
        }

        let response_text = response.text().await?;
        tracing::debug!("settings {} {} response: {}", method, path, response_text);

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

    /// Make an authenticated GET request to the API
    ///
    /// This method handles authentication, sends the request, and parses the response.
    ///
    /// # Arguments
    ///
    /// * `url` - Full URL to request
    ///
    /// # Errors
    ///
    /// Returns an error if authentication fails, request fails, or response parsing fails
    async fn authenticated_get<T>(&self, url: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        const METHOD: &str = "GET";

        // Extract URL for authentication (without query parameters)
        let url_for_auth = crate::npubcash::extract_auth_url(url)?;

        // Get authentication token
        let auth_token = self
            .auth_provider
            .get_auth_token(&url_for_auth, METHOD)
            .await?;

        // Send the HTTP request with authentication headers
        tracing::debug!("Making {} request to {}", METHOD, url);
        let response = self
            .http_client
            .get(url)
            .header("Authorization", auth_token)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                concat!("cdk-nostr/", env!("CARGO_PKG_VERSION")),
            )
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

#[cfg(test)]
mod tests {
    use nostr_sdk::prelude::Keys;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn get_missing_quotes_empty_ids_short_circuits() {
        let base_url = "http://127.0.0.1:1".to_string();
        let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), Keys::generate()));
        let client = NpubCashClient::new(base_url, auth_provider);

        // No request may be attempted for an empty ID list; the unroutable
        // address would otherwise fail.
        let quotes = client
            .get_missing_quotes(&[])
            .await
            .expect("empty ID list resolves without a request");

        assert!(quotes.is_empty());
    }

    #[tokio::test]
    async fn set_quote_locking_patches_user_lock_with_camel_case_payload() {
        let (base_url, server) = start_settings_server().await;
        let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), Keys::generate()));
        let client = NpubCashClient::new(base_url, auth_provider);

        client
            .set_quote_locking(true)
            .await
            .expect("set_quote_locking succeeds against mock server");

        let request = server.await.expect("server task completes");
        assert!(
            request.starts_with("PATCH /api/v2/user/lock HTTP/1.1"),
            "unexpected request line: {}",
            request.lines().next().unwrap_or_default()
        );
        assert!(
            request.contains("\"lockQuotes\":true"),
            "payload must use the server's camelCase field: {}",
            request
        );
    }

    #[tokio::test]
    async fn set_quote_locking_falls_back_to_documented_endpoint_on_404() {
        let (base_url, server) = start_fallback_settings_server().await;
        let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), Keys::generate()));
        let client = NpubCashClient::new(base_url, auth_provider);

        let response = client
            .set_quote_locking(true)
            .await
            .expect("fallback to documented endpoint succeeds");

        assert!(response.data.user().lock_quote);

        let requests = server.await.expect("server task completes");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("PATCH /api/v2/user/lock HTTP/1.1"));
        assert!(requests[1].starts_with("PUT /api/v2/settings/lock HTTP/1.1"));
        assert!(requests[1].contains("\"lockQuotes\":true"));
    }

    #[tokio::test]
    async fn user_settings_parse_documented_flat_layout() {
        // npub.cash API docs layout: data is not wrapped in `user` and the
        // flag is called `lockQuotes`.
        let response: crate::npubcash::types::UserResponse = serde_json::from_str(
            r#"{"error":false,"data":{"pubkey":"npub1test","mintUrl":"https://mint.example.com","lockQuotes":true}}"#,
        )
        .expect("flat docs layout parses");
        assert!(response.data.user().lock_quote);
        assert_eq!(
            response.data.user().mint_url.as_deref(),
            Some("https://mint.example.com")
        );
    }

    async fn start_settings_server() -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("test server has local addr");
        let base_url = format!("http://{}", addr);

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("connection accepted");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 2048];

            loop {
                let read = stream.read(&mut chunk).await.expect("request is readable");
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    // header end found; the small JSON body fits in this buffer
                    if String::from_utf8_lossy(&buffer).contains("\"lockQuotes\":") {
                        break;
                    }
                }
            }

            let body = r#"{"error":false,"data":{"user":{"pubkey":"test","mintUrl":"https://mint.example.com","lockQuote":true}}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response is written");

            String::from_utf8_lossy(&buffer).to_string()
        });

        (base_url, server)
    }

    /// Serves 404 for the first request and the docs-layout settings response
    /// for the second.
    async fn start_fallback_settings_server() -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("test server has local addr");
        let base_url = format!("http://{}", addr);

        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            let bodies = [
                (404, r#"{"error":true,"message":"Not found"}"#),
                (
                    200,
                    r#"{"error":false,"data":{"pubkey":"npub1test","mintUrl":"https://mint.example.com","lockQuotes":true}}"#,
                ),
            ];

            for (status, body) in bodies {
                let (mut stream, _) = listener.accept().await.expect("connection accepted");
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 2048];

                loop {
                    let read = stream.read(&mut chunk).await.expect("request is readable");
                    if read == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.windows(4).any(|w| w == b"\r\n\r\n")
                        && String::from_utf8_lossy(&buffer).contains("\"lockQuotes\":")
                    {
                        break;
                    }
                }

                requests.push(String::from_utf8_lossy(&buffer).to_string());

                let status_line = if status == 200 {
                    "200 OK"
                } else {
                    "404 Not Found"
                };
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("response is written");
            }

            requests
        });

        (base_url, server)
    }
}
