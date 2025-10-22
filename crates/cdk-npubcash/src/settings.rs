//! User settings management

use std::sync::Arc;

use reqwest::Client as HttpClient;
use serde::Serialize;
use tracing::instrument;

use crate::auth::AuthProvider;
use crate::error::Result;
use crate::types::UserResponse;
use crate::Error;

const SETTINGS_PATH: &str = "/api/v2/user/settings";

/// Manager for user settings in NpubCash
pub struct SettingsManager {
    base_url: String,
    auth_provider: Arc<dyn AuthProvider>,
    http_client: HttpClient,
}

impl std::fmt::Debug for SettingsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsManager")
            .field("base_url", &self.base_url)
            .field("auth_provider", &self.auth_provider)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct MintUrlPayload {
    mint_url: String,
}

impl SettingsManager {
    /// Create a new settings manager
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the NpubCash service
    /// * `auth_provider` - Authentication provider for signing requests
    pub fn new(base_url: String, auth_provider: Arc<dyn AuthProvider>) -> Self {
        Self {
            base_url,
            auth_provider,
            http_client: HttpClient::new(),
        }
    }

    /// Set the mint URL for the user
    ///
    /// # Arguments
    ///
    /// * `mint_url` - URL of the Cashu mint to use
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    #[instrument(skip(self, mint_url))]
    pub async fn set_mint_url(&self, mint_url: impl Into<String>) -> Result<UserResponse> {
        let url = format!("{}{}", self.base_url, SETTINGS_PATH);
        let payload = MintUrlPayload {
            mint_url: mint_url.into(),
        };
        self.update_settings(&url, &payload).await
    }

    async fn update_settings<T: Serialize + Sync>(
        &self,
        url: &str,
        payload: &T,
    ) -> Result<UserResponse> {
        let parsed_url = url::Url::parse(url)?;
        let url_for_auth = format!(
            "{}://{}{}",
            parsed_url.scheme(),
            parsed_url
                .host_str()
                .ok_or_else(|| Error::Custom("Invalid URL: missing host".to_string()))?,
            parsed_url.path()
        );
        let auth_token = self
            .auth_provider
            .get_auth_token(&url_for_auth, "POST")
            .await?;

        let response = self
            .http_client
            .post(url)
            .header("Authorization", auth_token)
            .json(payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(Error::Api {
                message: error_text,
                status: status.as_u16(),
            });
        }

        Ok(response.json().await?)
    }
}
