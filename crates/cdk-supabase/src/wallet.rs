use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, AeadCore, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::rand::rngs::OsRng;
use cdk_common::auth::oidc::OidcClient;
use cdk_common::common::ProofInfo;
use cdk_common::database::wallet::Database;
use cdk_common::database::{Error as DatabaseError, KVStoreDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{
    CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use cdk_common::secret::Secret;
use cdk_common::util::hex;
use cdk_common::wallet::{
    self, MintQuote, Transaction, TransactionDirection, TransactionId, WalletSaga,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use url::Url;

use crate::Error;

#[rustfmt::skip]
mod migrations {
    include!(concat!(env!("OUT_DIR"), "/migrations_supabase.rs"));
}

/// Returns the concatenated SQL of all migration files.
///
/// Operators can use this to set up the database manually via the Supabase
/// Dashboard SQL editor or `supabase db push`.
pub(crate) fn get_schema_sql_inner() -> String {
    migrations::MIGRATIONS
        .iter()
        .map(|(_, _, sql)| *sql)
        .collect::<Vec<&str>>()
        .join("\n\n")
}

/// URL-encode a value for use in query parameters
fn url_encode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

/// Decode JWT expiration from token string (without verification)
fn decode_jwt_expiry(token: &str) -> Option<u64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_part = parts[1];

    use bitcoin::base64::engine::general_purpose;
    use bitcoin::base64::Engine as _;

    let decoded = general_purpose::URL_SAFE_NO_PAD.decode(payload_part).ok()?;

    #[derive(Deserialize)]
    struct Claims {
        exp: Option<u64>,
    }

    let claims: Claims = serde_json::from_slice(&decoded).ok()?;
    claims.exp
}

/// Authentication provider for Supabase
///
/// This enum abstracts the token refresh logic for different authentication methods.
#[derive(Debug, Clone)]
pub enum AuthProvider {
    /// No authentication provider - uses API key only, no automatic token refresh
    None,
    /// Supabase Auth (GoTrue) - uses Supabase's built-in authentication
    ///
    /// Token refresh uses `POST /auth/v1/token` with `grant_type=refresh_token`
    SupabaseAuth,
    /// External OIDC provider - uses standard OIDC discovery and token endpoint
    Oidc(OidcClient),
}

/// Response from Supabase Auth token refresh
#[derive(Debug, Deserialize)]
struct SupabaseTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    #[serde(skip)]
    _token_type: (),
}

/// Supabase wallet database implementation
///
/// This database uses two types of authentication:
/// - `api_key`: The Supabase project API key (required, used in `apikey` header)
/// - `jwt_token`: An optional JWT token for user authentication (used in `Authorization: Bearer` header)
///
/// When `jwt_token` is set, requests will include both headers:
/// - `apikey: <api_key>`
/// - `Authorization: Bearer <jwt_token>`
///
/// When `jwt_token` is not set, the `api_key` is used for both headers (legacy behavior).
///
/// ## Authentication Providers
///
/// The database supports multiple authentication providers via [`AuthProvider`]:
/// - **None**: No automatic token refresh, use API key only
/// - **SupabaseAuth**: Uses Supabase's GoTrue API for token refresh
/// - **Oidc**: Uses an external OIDC provider for token refresh
#[derive(Debug, Clone)]
pub struct SupabaseWalletDatabase {
    url: Url,
    api_key: String,
    jwt_token: Arc<RwLock<Option<String>>>,
    refresh_token: Arc<RwLock<Option<String>>>,
    token_expiration: Arc<RwLock<Option<u64>>>,
    auth_provider: Arc<RwLock<AuthProvider>>,
    client: Client,
    encryption_key: Arc<RwLock<Option<Key<Aes256Gcm>>>>,
}

impl SupabaseWalletDatabase {
    /// Create a new SupabaseWalletDatabase with API key only (legacy behavior)
    ///
    /// No automatic token refresh is configured.
    ///
    /// **Note**: This does NOT run or check migrations automatically. After
    /// authentication, call [`check_schema_compatibility()`] to verify the
    /// database schema is ready. Migrations must be run separately by an
    /// administrator — see [`get_schema_sql()`] or use `supabase db push`.
    pub async fn new(url: Url, api_key: String) -> Result<Self, Error> {
        Ok(Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            auth_provider: Arc::new(RwLock::new(AuthProvider::None)),
            client: Client::new(),
            encryption_key: Arc::new(RwLock::new(None)),
        })
    }

    /// Create a new SupabaseWalletDatabase with Supabase Auth for token refresh
    ///
    /// This uses Supabase's built-in GoTrue authentication system.
    /// Token refresh uses `POST /auth/v1/token` with `grant_type=refresh_token`.
    ///
    /// **Note**: This does NOT run or check migrations automatically. After
    /// authentication, call [`check_schema_compatibility()`] to verify the
    /// database schema is ready. Migrations must be run separately by an
    /// administrator — see [`get_schema_sql()`] or use `supabase db push`.
    pub async fn with_supabase_auth(url: Url, api_key: String) -> Result<Self, Error> {
        Ok(Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            auth_provider: Arc::new(RwLock::new(AuthProvider::SupabaseAuth)),
            client: Client::new(),
            encryption_key: Arc::new(RwLock::new(None)),
        })
    }

    /// Create a new SupabaseWalletDatabase with external OIDC client for auth
    ///
    /// This uses an external OIDC provider (e.g., Keycloak, Auth0) for token refresh.
    /// The OIDC provider must be configured in Supabase to validate the JWTs.
    ///
    /// **Note**: This does NOT run or check migrations automatically. After
    /// authentication, call [`check_schema_compatibility()`] to verify the
    /// database schema is ready. Migrations must be run separately by an
    /// administrator — see [`get_schema_sql()`] or use `supabase db push`.
    pub async fn with_oidc(
        url: Url,
        api_key: String,
        oidc_client: OidcClient,
    ) -> Result<Self, Error> {
        Ok(Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            auth_provider: Arc::new(RwLock::new(AuthProvider::Oidc(oidc_client))),
            client: Client::new(),
            encryption_key: Arc::new(RwLock::new(None)),
        })
    }

    /// The schema version required by this SDK version.
    ///
    /// This must match the latest `schema_version` value set in the migration files.
    /// When adding new migrations, update this constant and set the same value
    /// in the new migration's `INSERT INTO schema_info` statement.
    pub const REQUIRED_SCHEMA_VERSION: u32 = 5;

    /// Get the full database schema SQL
    ///
    /// Returns the concatenated SQL of all migration files.
    ///
    /// Use this to set up or update the database schema by running the output
    /// through the Supabase Dashboard SQL editor or `supabase db push`.
    /// This is an **admin-only operation** — never run this from a client app.
    pub fn get_schema_sql() -> String {
        get_schema_sql_inner()
    }

    /// Check that the database schema is compatible with this SDK version
    ///
    /// This is the **recommended client-side startup check**. It queries the
    /// `schema_info` table (which is readable by all authenticated users) to
    /// verify the database has the required schema version.
    ///
    /// # Errors
    ///
    /// - [`Error::SchemaNotInitialized`] if the `schema_info` table doesn't exist
    ///   (database was never set up or is running a pre-v4 schema).
    /// - [`Error::SchemaMismatch`] if the database schema version is older than
    ///   what this SDK version requires.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Call after authentication, before using the database
    /// db.check_schema_compatibility().await?;
    /// // Database is ready for use
    /// ```
    pub async fn check_schema_compatibility(&self) -> Result<(), Error> {
        let path = "rest/v1/schema_info?key=eq.schema_version&select=value";

        let result = self.get_request(path).await;

        match result {
            Ok((status, text)) => {
                if status == StatusCode::NOT_FOUND
                    || text.contains("relation")
                    || text.contains("does not exist")
                {
                    return Err(Error::SchemaNotInitialized);
                }

                if !status.is_success() {
                    // If we get a 404-like error or permission error, schema_info
                    // table likely doesn't exist
                    return Err(Error::SchemaNotInitialized);
                }

                // Parse the response: [{"value": "4"}] or []
                let items: Vec<serde_json::Value> =
                    serde_json::from_str(&text).map_err(|_| Error::SchemaNotInitialized)?;

                if items.is_empty() {
                    return Err(Error::SchemaNotInitialized);
                }

                let version_str = items[0]
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or(Error::SchemaNotInitialized)?;

                let found_version: u32 = version_str
                    .parse()
                    .map_err(|_| Error::SchemaNotInitialized)?;

                if found_version < Self::REQUIRED_SCHEMA_VERSION {
                    return Err(Error::SchemaMismatch {
                        required: Self::REQUIRED_SCHEMA_VERSION,
                        found: found_version,
                    });
                }

                tracing::info!(
                    schema_version = found_version,
                    required = Self::REQUIRED_SCHEMA_VERSION,
                    "Database schema compatibility check passed"
                );

                Ok(())
            }
            Err(_) => Err(Error::SchemaNotInitialized),
        }
    }

    /// Set or update the JWT token for authentication
    pub async fn set_jwt_token(&self, token: Option<String>) {
        let mut jwt = self.jwt_token.write().await;
        *jwt = token.clone();

        let mut expiration = self.token_expiration.write().await;

        if let Some(t) = token {
            *expiration = decode_jwt_expiry(&t);
        } else {
            *expiration = None;
        }
    }

    /// Set refresh token
    pub async fn set_refresh_token(&self, token: Option<String>) {
        let mut refresh = self.refresh_token.write().await;
        *refresh = token;
    }

    /// Derives an AES-256-GCM encryption key from `password` via SHA-256.
    pub async fn set_encryption_password(&self, password: &str) {
        let key = sha256::Hash::hash(password.as_bytes());

        let mut encryption_key = self.encryption_key.write().await;
        *encryption_key = Some(*Key::<Aes256Gcm>::from_slice(key.as_byte_array()));
    }

    async fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, DatabaseError> {
        let key_guard = self.encryption_key.read().await;
        let key = key_guard
            .as_ref()
            .ok_or(DatabaseError::Internal("Encryption key not set".into()))?;
        let cipher = Aes256Gcm::new(key);
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 96-bits; unique per message
        let ciphertext = cipher
            .encrypt(&nonce, data)
            .map_err(|_| DatabaseError::Internal("Encryption failed".into()))?;

        // Prepend nonce to ciphertext
        let mut result = nonce.to_vec();
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    async fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, DatabaseError> {
        let key_guard = self.encryption_key.read().await;
        let key = key_guard
            .as_ref()
            .ok_or(DatabaseError::Internal("Encryption key not set".into()))?;
        let cipher = Aes256Gcm::new(key);

        if data.len() < 12 {
            return Err(DatabaseError::Internal("Invalid ciphertext length".into()));
        }

        let nonce = Nonce::from_slice(&data[0..12]);
        let ciphertext = &data[12..];

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| DatabaseError::Internal("Decryption failed".into()))
    }

    async fn decrypt_proof_table(&self, p: &mut ProofTable) {
        // Decrypt secret
        if let Ok(encrypted_bytes) = hex::decode(&p.secret) {
            if let Ok(decrypted) = self.decrypt(&encrypted_bytes).await {
                if let Ok(secret_str) = String::from_utf8(decrypted) {
                    p.secret = secret_str;
                }
            }
        }

        // Decrypt C
        if let Ok(encrypted_c) = hex::decode(&p.c) {
            if let Ok(decrypted_c) = self.decrypt(&encrypted_c).await {
                p.c = hex::encode(decrypted_c);
            }
        }
    }

    /// Refresh the access token using the stored refresh token
    ///
    /// This method handles different authentication providers:
    /// - **SupabaseAuth**: Uses `POST /auth/v1/token` with `grant_type=refresh_token`
    /// - **Oidc**: Uses the OIDC provider's token endpoint
    /// - **None**: Returns an error (no provider configured)
    pub async fn refresh_access_token(&self) -> Result<(), Error> {
        let refresh_token = self.refresh_token.read().await.clone();
        let refresh = refresh_token
            .ok_or_else(|| Error::Supabase("No refresh token available".to_string()))?;

        let auth_provider = self.auth_provider.read().await.clone();

        match auth_provider {
            AuthProvider::None => {
                return Err(Error::Supabase(
                    "No authentication provider configured".to_string(),
                ));
            }
            AuthProvider::SupabaseAuth => {
                // Use Supabase GoTrue API for token refresh
                let auth_url = self
                    .url
                    .join("auth/v1/token?grant_type=refresh_token")
                    .map_err(|e| Error::Supabase(format!("Invalid auth URL: {}", e)))?;

                let body = serde_json::json!({
                    "refresh_token": refresh
                });

                let response = self
                    .client
                    .post(auth_url)
                    .header("apikey", &self.api_key)
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(Error::Reqwest)?;

                let status = response.status();
                if !status.is_success() {
                    let text = response.text().await.unwrap_or_default();
                    return Err(Error::Supabase(format!(
                        "Supabase token refresh failed: HTTP {} - {}",
                        status, text
                    )));
                }

                let token_response: SupabaseTokenResponse =
                    response.json().await.map_err(Error::Reqwest)?;

                self.set_jwt_token(Some(token_response.access_token)).await;

                if let Some(new_refresh) = token_response.refresh_token {
                    self.set_refresh_token(Some(new_refresh)).await;
                }

                if let Some(expires_in) = token_response.expires_in {
                    let expiration = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| Error::Supabase(format!("SystemTime error: {}", e)))?
                        .as_secs()
                        + expires_in as u64;
                    let mut exp = self.token_expiration.write().await;
                    *exp = Some(expiration);
                }
            }
            AuthProvider::Oidc(oidc) => {
                let client_id = oidc.client_id().ok_or_else(|| {
                    Error::Supabase("Client ID not set in OIDC client".to_string())
                })?;

                let response = oidc
                    .refresh_access_token(client_id, refresh)
                    .await
                    .map_err(|e| Error::Supabase(e.to_string()))?;

                self.set_jwt_token(Some(response.access_token)).await;

                if let Some(new_refresh) = response.refresh_token {
                    self.set_refresh_token(Some(new_refresh)).await;
                }

                if let Some(expires_in) = response.expires_in {
                    let expiration = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| Error::Supabase(format!("SystemTime error: {}", e)))?
                        .as_secs()
                        + expires_in as u64;
                    let mut exp = self.token_expiration.write().await;
                    *exp = Some(expiration);
                }
            }
        }

        Ok(())
    }

    /// Sign up a new user and automatically set tokens if returned
    pub async fn signup(&self, email: &str, password: &str) -> Result<SupabaseAuthResponse, Error> {
        let response = SupabaseAuth::signup(&self.url, &self.api_key, email, password).await?;

        // If signup returns valid tokens (e.g. auto-confirm enabled), set them
        if !response.access_token.is_empty() {
            self.set_jwt_token(Some(response.access_token.clone()))
                .await;
        }
        if let Some(refresh) = &response.refresh_token {
            self.set_refresh_token(Some(refresh.clone())).await;
        }

        Ok(response)
    }

    /// Sign in a user and automatically set tokens on the database instance
    pub async fn signin(&self, email: &str, password: &str) -> Result<SupabaseAuthResponse, Error> {
        let response = SupabaseAuth::signin(&self.url, &self.api_key, email, password).await?;

        self.set_jwt_token(Some(response.access_token.clone()))
            .await;
        if let Some(refresh) = &response.refresh_token {
            self.set_refresh_token(Some(refresh.clone())).await;
        }
        if let Some(expires_in) = response.expires_in {
            let expiration = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| Error::Supabase(format!("SystemTime error: {}", e)))?
                .as_secs()
                + expires_in as u64;
            let mut exp = self.token_expiration.write().await;
            *exp = Some(expiration);
        }

        Ok(response)
    }

    /// Get the current JWT token if set
    pub async fn get_jwt_token(&self) -> Option<String> {
        self.jwt_token.read().await.clone()
    }

    /// Call a Supabase RPC function with JSON parameters
    pub async fn call_rpc(&self, function_name: &str, params_json: &str) -> Result<String, Error> {
        // Parse the JSON to validate it and convert to Value for sending
        // Treat empty string as empty object for convenience
        let params: serde_json::Value = if params_json.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(params_json).map_err(Error::Serde)?
        };

        let path = format!("rest/v1/rpc/{}", function_name);
        let url = self.join_url(&path)?;
        let auth_bearer = self.get_auth_bearer().await;

        let res = self
            .client
            .post(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Content-Type", "application/json")
            .json(&params)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        if !status.is_success() {
            return Err(Error::Supabase(format!(
                "RPC '{}' failed: HTTP {} - {}",
                function_name, status, text
            )));
        }

        Ok(text)
    }

    /// Get the authorization token to use for requests
    ///
    /// Returns the JWT token if set, otherwise falls back to the API key.
    async fn get_auth_bearer(&self) -> String {
        // Check expiration
        let expiration = *self.token_expiration.read().await;
        if let Some(exp) = expiration {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("SystemTime should be after UNIX_EPOCH")
                .as_secs();
            // Refresh if expired or expiring in 60 seconds
            if now + 60 > exp {
                if let Err(e) = self.refresh_access_token().await {
                    tracing::warn!("Failed to refresh token: {}", e);
                }
            }
        }

        self.jwt_token
            .read()
            .await
            .clone()
            .unwrap_or_else(|| self.api_key.clone())
    }

    /// Join the base URL with a path
    pub fn join_url(&self, path: &str) -> Result<Url, DatabaseError> {
        self.url
            .join(path)
            .map_err(|e| DatabaseError::Internal(e.to_string()))
    }

    /// Make a GET request and return the response text
    async fn get_request(&self, path: &str) -> Result<(StatusCode, String), Error> {
        let url = self.join_url(path)?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "GET", url = %url, "Supabase request");

        let res = self
            .client
            .get(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "GET", url = %url, status = %status, response_len = text.len(), "Supabase response");

        Ok((status, text))
    }

    /// Make a POST request with JSON body
    async fn post_request<T: Serialize + Debug>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<(StatusCode, String), Error> {
        let url = self.join_url(path)?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "POST", url = %url, "Supabase request");

        let res = self
            .client
            .post(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Prefer", "resolution=merge-duplicates,missing=default")
            .json(body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "POST", url = %url, status = %status, response_len = text.len(), "Supabase response");

        Ok((status, text))
    }

    /// Make a PATCH request with JSON body
    async fn patch_request<T: Serialize + Debug>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<(StatusCode, String), Error> {
        let url = self.join_url(path)?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "PATCH", url = %url, "Supabase request");

        let res = self
            .client
            .patch(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .json(body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "PATCH", url = %url, status = %status, response_len = text.len(), "Supabase response");

        Ok((status, text))
    }

    /// Make a PATCH request and ask PostgREST to return the updated rows as JSON
    /// (`Prefer: return=representation`).  Returns `(status, body)` where body is
    /// an empty JSON array `[]` when the filter matched no rows.
    async fn patch_request_returning<T: Serialize + Debug>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<(StatusCode, String), Error> {
        let url = self.join_url(path)?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "PATCH", url = %url, "Supabase request (returning)");

        let res = self
            .client
            .patch(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Prefer", "return=representation")
            .json(body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "PATCH", url = %url, status = %status, response_len = text.len(), "Supabase response (returning)");

        Ok((status, text))
    }

    /// Make a DELETE request
    async fn delete_request(&self, path: &str) -> Result<(StatusCode, String), Error> {
        let url = self.join_url(path)?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "DELETE", url = %url, "Supabase request");

        let res = self
            .client
            .delete(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "DELETE", url = %url, status = %status, response_len = text.len(), "Supabase response");

        Ok((status, text))
    }

    /// Parse a JSON response, returning None for empty responses
    fn parse_response<T: serde::de::DeserializeOwned>(text: &str) -> Result<Option<Vec<T>>, Error> {
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let items: Vec<T> = serde_json::from_str(text).map_err(Error::Serde)?;
        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(items))
        }
    }
}

#[async_trait]
impl KVStoreDatabase for SupabaseWalletDatabase {
    type Err = DatabaseError;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        let path = format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}&key=eq.{}",
            url_encode(primary_namespace),
            url_encode(secondary_namespace),
            url_encode(key)
        );

        let (status, text) = self.get_request(&path).await?;

        if status == StatusCode::NO_CONTENT || !status.is_success() {
            if !status.is_success() && status != StatusCode::NO_CONTENT {
                return Err(DatabaseError::Internal(format!(
                    "kv_read failed: HTTP {}",
                    status
                )));
            }
            return Ok(None);
        }

        if let Some(items) = Self::parse_response::<KVStoreTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                let bytes = hex::decode(item.value)
                    .map_err(|_| DatabaseError::Internal("Invalid hex in kv_store".into()))?;

                // Decrypt value
                let decrypted = self.decrypt(&bytes).await?;
                return Ok(Some(decrypted));
            }
        }
        Ok(None)
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err> {
        let path = format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}",
            url_encode(primary_namespace),
            url_encode(secondary_namespace)
        );

        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "kv_list failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<KVStoreTable>(&text)? {
            Ok(items.into_iter().map(|i| i.key).collect())
        } else {
            Ok(Vec::new())
        }
    }
}
#[async_trait]
impl Database<DatabaseError> for SupabaseWalletDatabase {
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, DatabaseError> {
        let path = format!(
            "rest/v1/mint?mint_url=eq.{}",
            url_encode(&mint_url.to_string())
        );
        let (status, text) = self.get_request(&path).await?;

        // 404 or empty result means not found
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_mint failed: HTTP {}",
                status
            )));
        }

        if let Some(mints) = Self::parse_response::<MintTable>(&text)? {
            if let Some(mint) = mints.into_iter().next() {
                return Ok(Some(mint.try_into()?));
            }
        }
        Ok(None)
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, DatabaseError> {
        let (status, text) = self.get_request("rest/v1/mint").await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_mints failed: HTTP {}",
                status
            )));
        }

        let mut map = HashMap::new();
        if let Some(mints) = Self::parse_response::<MintTable>(&text)? {
            for mint in mints {
                map.insert(
                    MintUrl::from_str(&mint.mint_url)
                        .map_err(|e| DatabaseError::Internal(e.to_string()))?,
                    Some(mint.try_into()?),
                );
            }
        }
        Ok(map)
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, DatabaseError> {
        let path = format!(
            "rest/v1/keyset?mint_url=eq.{}",
            url_encode(&mint_url.to_string())
        );
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_mint_keysets failed: HTTP {}",
                status
            )));
        }

        if let Some(keysets) = Self::parse_response::<KeySetTable>(&text)? {
            let result: Result<Vec<KeySetInfo>, _> =
                keysets.into_iter().map(|ks| ks.try_into()).collect();
            Ok(Some(result?))
        } else {
            Ok(None)
        }
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, DatabaseError> {
        let path = format!(
            "rest/v1/keyset?id=eq.{}",
            url_encode(&keyset_id.to_string())
        );
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_keyset_by_id failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<KeySetTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                return Ok(Some(item.try_into()?));
            }
        }
        Ok(None)
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, DatabaseError> {
        let path = format!("rest/v1/mint_quote?id=eq.{}", url_encode(quote_id));
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_mint_quote failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<MintQuoteTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                return Ok(Some(item.try_into()?));
            }
        }
        Ok(None)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, DatabaseError> {
        let (status, text) = self.get_request("rest/v1/mint_quote").await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_mint_quotes failed: HTTP {}",
                status
            )));
        }

        if let Some(quotes) = Self::parse_response::<MintQuoteTable>(&text)? {
            quotes.into_iter().map(|q| q.try_into()).collect()
        } else {
            Ok(Vec::new())
        }
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, DatabaseError> {
        let (status, text) = self
            .get_request("rest/v1/mint_quote?amount_issued=eq.0")
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_unissued_mint_quotes failed: HTTP {}",
                status
            )));
        }

        if let Some(quotes) = Self::parse_response::<MintQuoteTable>(&text)? {
            quotes.into_iter().map(|q| q.try_into()).collect()
        } else {
            Ok(Vec::new())
        }
    }

    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, DatabaseError> {
        let path = format!("rest/v1/melt_quote?id=eq.{}", url_encode(quote_id));
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_melt_quote failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<MeltQuoteTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                return Ok(Some(item.try_into()?));
            }
        }
        Ok(None)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, DatabaseError> {
        let (status, text) = self.get_request("rest/v1/melt_quote").await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_melt_quotes failed: HTTP {}",
                status
            )));
        }

        if let Some(quotes) = Self::parse_response::<MeltQuoteTable>(&text)? {
            quotes.into_iter().map(|q| q.try_into()).collect()
        } else {
            Ok(Vec::new())
        }
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, DatabaseError> {
        let path = format!("rest/v1/key?id=eq.{}", url_encode(&id.to_string()));
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_keys failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<KeyTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                return Ok(Some(item.try_into()?));
            }
        }
        Ok(None)
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, DatabaseError> {
        let mut query = String::from("rest/v1/proof?select=*");
        if let Some(url) = mint_url {
            query.push_str(&format!("&mint_url=eq.{}", url_encode(&url.to_string())));
        }
        if let Some(u) = unit {
            query.push_str(&format!("&unit=eq.{}", url_encode(&u.to_string())));
        }
        if let Some(states) = state {
            let s_str: Vec<String> = states.iter().map(|s| s.to_string()).collect();
            query.push_str(&format!("&state=in.({})", s_str.join(",")));
        }

        let (status, text) = self.get_request(&query).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_proofs failed: HTTP {}",
                status
            )));
        }

        let mut result = Vec::new();
        if let Some(proofs) = Self::parse_response::<ProofTable>(&text)? {
            for mut p in proofs {
                self.decrypt_proof_table(&mut p).await;

                result.push(p.try_into()?);
            }
        }

        // Filter by spending conditions in memory if specified
        if let Some(conds) = spending_conditions {
            result.retain(|p: &ProofInfo| {
                if let Some(sc) = &p.spending_condition {
                    conds.contains(sc)
                } else {
                    false
                }
            });
        }

        Ok(result)
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, DatabaseError> {
        if ys.is_empty() {
            return Ok(Vec::new());
        }

        let ys_str: Vec<String> = ys.iter().map(|y| hex::encode(y.to_bytes())).collect();
        let filter = format!("({})", ys_str.join(","));
        let path = format!("rest/v1/proof?y=in.{}", filter);

        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_proofs_by_ys failed: HTTP {}",
                status
            )));
        }

        if let Some(proofs) = Self::parse_response::<ProofTable>(&text)? {
            let mut result = Vec::new();
            for mut p in proofs {
                self.decrypt_proof_table(&mut p).await;

                result.push(p.try_into()?);
            }
            Ok(result)
        } else {
            Ok(Vec::new())
        }
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, DatabaseError> {
        // Note: Ideally this would use a server-side SUM aggregation, but PostgREST
        // doesn't support aggregate functions directly. We fetch all proofs and sum locally.
        let proofs = self.get_proofs(mint_url, unit, state, None).await?;
        Ok(proofs.iter().map(|p| p.proof.amount.to_u64()).sum())
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, DatabaseError> {
        let id_hex = transaction_id.to_string();
        let path = format!("rest/v1/transactions?id=eq.\\x{}", id_hex);

        let (status, text) = self.get_request(&path).await?;

        // 404 or empty result means not found
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_transaction failed: HTTP {}",
                status
            )));
        }

        if let Some(txs) = Self::parse_response::<TransactionTable>(&text)? {
            if let Some(t) = txs.into_iter().next() {
                return Ok(Some(t.try_into()?));
            }
        }
        Ok(None)
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, DatabaseError> {
        let mut query = String::from("rest/v1/transactions?select=*");
        if let Some(url) = mint_url {
            query.push_str(&format!("&mint_url=eq.{}", url_encode(&url.to_string())));
        }
        if let Some(d) = direction {
            query.push_str(&format!("&direction=eq.{}", url_encode(&d.to_string())));
        }
        if let Some(u) = unit {
            query.push_str(&format!("&unit=eq.{}", url_encode(&u.to_string())));
        }

        let (status, text) = self.get_request(&query).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "list_transactions failed: HTTP {}",
                status
            )));
        }

        if let Some(txs) = Self::parse_response::<TransactionTable>(&text)? {
            txs.into_iter().map(|t| t.try_into()).collect()
        } else {
            Ok(Vec::new())
        }
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), DatabaseError> {
        // If nothing to do, return early
        if added.is_empty() && removed_ys.is_empty() {
            return Ok(());
        }

        // Convert proofs to table format for the RPC call

        // Re-do serialization loop properly to allow await
        let mut proofs_json: Vec<serde_json::Value> = Vec::with_capacity(added.len());
        for p in added {
            let mut table: ProofTable = p.try_into()?;

            // Encrypt secret
            let secret_bytes = table.secret.as_bytes();
            let encrypted = self.encrypt(secret_bytes).await?;
            table.secret = hex::encode(encrypted);

            // Encrypt C
            if let Ok(c_bytes) = hex::decode(&table.c) {
                let encrypted_c = self.encrypt(&c_bytes).await?;
                table.c = hex::encode(encrypted_c);
            }

            proofs_json.push(serde_json::to_value(&table).map_err(DatabaseError::from)?);
        }

        // Convert Y values to hex strings
        let ys_json: Vec<String> = removed_ys
            .iter()
            .map(|y| hex::encode(y.to_bytes()))
            .collect();

        // Try atomic RPC first
        let rpc_body = serde_json::json!({
            "p_proofs_to_add": proofs_json,
            "p_ys_to_remove": ys_json
        });

        let url = self.join_url("rest/v1/rpc/update_proofs_atomic")?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(
            method = "POST",
            url = %url,
            proofs_count = proofs_json.len(),
            remove_count = ys_json.len(),
            "Supabase atomic update_proofs RPC"
        );

        let res = self
            .client
            .post(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Content-Type", "application/json")
            .json(&rpc_body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(
            method = "POST",
            url = %url,
            status = %status,
            response_len = text.len(),
            "Supabase atomic update_proofs response"
        );

        if status.is_success() {
            return Ok(());
        }

        Err(DatabaseError::Internal(format!(
            "update_proofs_atomic RPC failed: HTTP {}. Ensure migrations have been run.",
            status
        )))
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), DatabaseError> {
        if ys.is_empty() {
            return Ok(());
        }

        let ys_str: Vec<String> = ys.iter().map(|y| hex::encode(y.to_bytes())).collect();
        let filter = format!("({})", ys_str.join(","));
        let path = format!("rest/v1/proof?y=in.{}", filter);

        let (status, response_text) = self
            .patch_request(&path, &serde_json::json!({ "state": state.to_string() }))
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_proofs_state failed: HTTP {} - {}",
                status, response_text
            )));
        }

        Ok(())
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), DatabaseError> {
        let item: TransactionTable = transaction.try_into()?;
        let (status, response_text) = self
            .post_request("rest/v1/transactions?on_conflict=id,wallet_id", &item)
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_transaction failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), DatabaseError> {
        let id_hex = transaction_id.to_string();
        let path = format!("rest/v1/transactions?id=eq.\\x{}", id_hex);

        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "remove_transaction failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), DatabaseError> {
        let old_encoded = url_encode(&old_mint_url.to_string());
        let update_body = serde_json::json!({ "mint_url": new_mint_url.to_string() });

        // Update mint table first (parent table)
        let path = format!("rest/v1/mint?mint_url=eq.{}", old_encoded);
        let (status, response_text) = self.patch_request(&path, &update_body).await?;
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_mint_url (mint) failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Update keyset table
        let path = format!("rest/v1/keyset?mint_url=eq.{}", old_encoded);
        let (status, response_text) = self.patch_request(&path, &update_body).await?;
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_mint_url (keyset) failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Update mint_quote table
        let path = format!("rest/v1/mint_quote?mint_url=eq.{}", old_encoded);
        let (status, response_text) = self.patch_request(&path, &update_body).await?;
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_mint_url (mint_quote) failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Update proof table
        let path = format!("rest/v1/proof?mint_url=eq.{}", old_encoded);
        let (status, response_text) = self.patch_request(&path, &update_body).await?;
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_mint_url (proof) failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Update transactions table
        let path = format!("rest/v1/transactions?mint_url=eq.{}", old_encoded);
        let (status, response_text) = self.patch_request(&path, &update_body).await?;
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_mint_url (transactions) failed: HTTP {} - {}",
                status, response_text
            )));
        }

        Ok(())
    }

    async fn increment_keyset_counter(
        &self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, DatabaseError> {
        // Use Supabase RPC for atomic increment
        // This calls the increment_keyset_counter PostgreSQL function
        let rpc_body = serde_json::json!({
            "p_keyset_id": keyset_id.to_string(),
            "p_increment": count as i32
        });

        let url = self.join_url("rest/v1/rpc/increment_keyset_counter")?;
        let auth_bearer = self.get_auth_bearer().await;

        tracing::debug!(method = "POST", url = %url, keyset_id = %keyset_id, increment = count, "Supabase RPC request");

        let res = self
            .client
            .post(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Content-Type", "application/json")
            .header("Prefer", "return=representation")
            .json(&rpc_body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "POST", url = %url, status = %status, response_len = text.len(), "Supabase RPC response");

        if status.is_success() {
            // RPC returns the new counter value directly
            let new_counter: i32 = serde_json::from_str(&text).map_err(|e| {
                DatabaseError::Internal(format!("Failed to parse counter response: {}", e))
            })?;
            return Ok(new_counter as u32);
        }

        Err(DatabaseError::Internal(format!(
            "increment_keyset_counter RPC failed: HTTP {}. Ensure migrations have been run.",
            status
        )))
    }

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), DatabaseError> {
        let info_table: MintTable = match mint_info {
            Some(info) => MintTable::from_info(mint_url.clone(), info)?,
            None => MintTable {
                mint_url: mint_url.to_string(),
                ..Default::default()
            },
        };

        let (status, response_text) = self
            .post_request("rest/v1/mint?on_conflict=mint_url,wallet_id", &info_table)
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_mint failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), DatabaseError> {
        let path = format!(
            "rest/v1/mint?mint_url=eq.{}",
            url_encode(&mint_url.to_string())
        );
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "remove_mint failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), DatabaseError> {
        if keysets.is_empty() {
            return Ok(());
        }

        let items: Result<Vec<KeySetTable>, DatabaseError> = keysets
            .into_iter()
            .map(|k| KeySetTable::from_info(mint_url.clone(), k))
            .collect();
        let items = items?;

        let (status, response_text) = self
            .post_request("rest/v1/keyset?on_conflict=id,wallet_id", &items)
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_mint_keysets failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), DatabaseError> {
        let expected_version = quote.version;
        let mut item: MintQuoteTable = quote.try_into()?;
        item.version = Some(expected_version.wrapping_add(1) as i32);

        let path = format!(
            "rest/v1/mint_quote?id=eq.{}&version=eq.{}",
            url_encode(&item.id),
            expected_version
        );

        // Use `return=representation` so PostgREST returns the updated rows as JSON.
        // An empty array `[]` means the version filter matched nothing — the row either
        // doesn't exist yet or was concurrently modified.
        let (status, response_text) = self.patch_request_returning(&path, &item).await?;

        if status.is_success() {
            let updated: serde_json::Value =
                serde_json::from_str(&response_text).unwrap_or(serde_json::Value::Null);
            let row_count = updated.as_array().map(|a| a.len()).unwrap_or(0);

            if row_count > 0 {
                // PATCH updated an existing row — done.
                return Ok(());
            }

            // No rows updated: the row doesn't exist yet — fall through to INSERT.
            let (status, response_text) = self
                .post_request("rest/v1/mint_quote?on_conflict=id,wallet_id", &item)
                .await?;

            if status.is_success() {
                return Ok(());
            }

            return Err(DatabaseError::Internal(format!(
                "add_mint_quote insert failed: HTTP {} - {}",
                status, response_text
            )));
        }

        Err(DatabaseError::Internal(format!(
            "add_mint_quote failed: HTTP {} - {}",
            status, response_text
        )))
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), DatabaseError> {
        let path = format!("rest/v1/mint_quote?id=eq.{}", url_encode(quote_id));
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "remove_mint_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), DatabaseError> {
        let expected_version = quote.version;
        let mut item: MeltQuoteTable = quote.try_into()?;
        item.version = Some(expected_version.wrapping_add(1) as i32);

        let path = format!(
            "rest/v1/melt_quote?id=eq.{}&version=eq.{}",
            url_encode(&item.id),
            expected_version
        );

        // Use `return=representation` so PostgREST returns the updated rows as JSON.
        // An empty array `[]` means the version filter matched nothing — the row either
        // doesn't exist yet or was concurrently modified.
        let (status, response_text) = self.patch_request_returning(&path, &item).await?;

        if status.is_success() {
            let updated: serde_json::Value =
                serde_json::from_str(&response_text).unwrap_or(serde_json::Value::Null);
            let row_count = updated.as_array().map(|a| a.len()).unwrap_or(0);

            if row_count > 0 {
                // PATCH updated an existing row — done.
                return Ok(());
            }

            // No rows updated: the row doesn't exist yet — fall through to INSERT.
            let (status, response_text) = self
                .post_request("rest/v1/melt_quote?on_conflict=id,wallet_id", &item)
                .await?;

            if status.is_success() {
                return Ok(());
            }

            return Err(DatabaseError::Internal(format!(
                "add_melt_quote insert failed: HTTP {} - {}",
                status, response_text
            )));
        }

        Err(DatabaseError::Internal(format!(
            "add_melt_quote failed: HTTP {} - {}",
            status, response_text
        )))
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), DatabaseError> {
        let path = format!("rest/v1/melt_quote?id=eq.{}", url_encode(quote_id));
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "remove_melt_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), DatabaseError> {
        keyset.verify_id().map_err(DatabaseError::from)?;
        let item = KeyTable::from_keyset(&keyset)?;

        let (status, response_text) = self
            .post_request("rest/v1/key?on_conflict=id,wallet_id", &item)
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_keys failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), DatabaseError> {
        let path = format!("rest/v1/key?id=eq.{}", url_encode(&id.to_string()));
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "remove_keys failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        // Encrypt value
        let encrypted = self.encrypt(value).await?;

        let item = KVStoreTable {
            primary_namespace: primary_namespace.to_string(),
            secondary_namespace: secondary_namespace.to_string(),
            key: key.to_string(),
            value: hex::encode(encrypted),
            _extra: Default::default(),
        };

        let (status, response_text) = self
            .post_request(
                "rest/v1/kv_store?on_conflict=primary_namespace,secondary_namespace,key,wallet_id",
                &item,
            )
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "kv_write failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), DatabaseError> {
        let path = format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}&key=eq.{}",
            url_encode(primary_namespace),
            url_encode(secondary_namespace),
            url_encode(key)
        );
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "kv_remove failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DatabaseError> {
        // Delegate to the KVStoreDatabase impl
        KVStoreDatabase::kv_read(self, primary_namespace, secondary_namespace, key).await
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        // Delegate to the KVStoreDatabase impl
        KVStoreDatabase::kv_list(self, primary_namespace, secondary_namespace).await
    }

    // ========== Saga methods ==========

    async fn add_saga(&self, saga: WalletSaga) -> Result<(), DatabaseError> {
        let saga_json = serde_json::to_string(&saga)
            .map_err(|e| DatabaseError::Internal(format!("Serialize saga: {e}")))?;

        let item = SagaTable {
            id: saga.id.to_string(),
            data: saga_json,
            version: saga.version as i32,
            completed: false,
            created_at: saga.created_at as i64,
            updated_at: saga.updated_at as i64,
            _extra: Default::default(),
        };

        let (status, response_text) = self
            .post_request("rest/v1/saga?on_conflict=id,wallet_id", &item)
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_saga failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn get_saga(&self, id: &uuid::Uuid) -> Result<Option<WalletSaga>, DatabaseError> {
        let path = format!("rest/v1/saga?id=eq.{}", url_encode(&id.to_string()));
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_saga failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<SagaTable>(&text)? {
            if let Some(item) = items.into_iter().next() {
                let saga: WalletSaga = serde_json::from_str(&item.data)
                    .map_err(|e| DatabaseError::Internal(format!("Deserialize saga: {e}")))?;
                return Ok(Some(saga));
            }
        }
        Ok(None)
    }

    async fn update_saga(&self, saga: WalletSaga) -> Result<bool, DatabaseError> {
        let expected_version = saga.version.saturating_sub(1);
        let saga_json = serde_json::to_string(&saga)
            .map_err(|e| DatabaseError::Internal(format!("Serialize saga: {e}")))?;

        let item = SagaTable {
            id: saga.id.to_string(),
            data: saga_json,
            version: saga.version as i32,
            completed: false,
            created_at: saga.created_at as i64,
            updated_at: saga.updated_at as i64,
            _extra: Default::default(),
        };

        // Use PostgREST filtering to only update if version matches (optimistic locking)
        let path = format!(
            "rest/v1/saga?id=eq.{}&version=eq.{}",
            url_encode(&saga.id.to_string()),
            expected_version
        );

        let (status, response_text) = self.patch_request(&path, &item).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "update_saga failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // PostgREST PATCH returns empty body for 0 rows updated. Check via GET.
        // Alternatively, we check if the response indicates changes were made.
        // A simpler approach: re-read and verify version was updated.
        let current = self.get_saga(&saga.id).await?;
        match current {
            Some(s) => Ok(s.version == saga.version),
            None => Ok(false),
        }
    }

    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), DatabaseError> {
        let path = format!("rest/v1/saga?id=eq.{}", url_encode(&id.to_string()));
        let (status, response_text) = self.delete_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "delete_saga failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn get_incomplete_sagas(&self) -> Result<Vec<WalletSaga>, DatabaseError> {
        let path = "rest/v1/saga?completed=eq.false&order=created_at.asc";
        let (status, text) = self.get_request(path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_incomplete_sagas failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<SagaTable>(&text)? {
            let mut sagas = Vec::new();
            for item in items {
                let saga: WalletSaga = serde_json::from_str(&item.data)
                    .map_err(|e| DatabaseError::Internal(format!("Deserialize saga: {e}")))?;
                sagas.push(saga);
            }
            Ok(sagas)
        } else {
            Ok(Vec::new())
        }
    }

    // ========== Proof reservation methods ==========

    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();
        for y in &ys {
            let y_hex = hex::encode(y.to_bytes());

            // Update proof state to Reserved with operation_id atomically by filtering on state=Unspent
            let update = serde_json::json!({
                "state": State::Reserved.to_string(),
                "used_by_operation": op_id_str,
            });

            // We filter on state=Unspent to ensure we only reserve proofs that are currently available.
            // This prevents race conditions where two operations try to reserve the same proof.
            let patch_path = format!(
                "rest/v1/proof?y=eq.{}&state=eq.{}",
                url_encode(&y_hex),
                url_encode(&State::Unspent.to_string())
            );

            let (status, response_text) = self.patch_request(&patch_path, &update).await?;

            if !status.is_success() {
                return Err(DatabaseError::Internal(format!(
                    "reserve_proofs: update failed: HTTP {} - {}",
                    status, response_text
                )));
            }

            // PostgREST returns 204 No Content for success.
            // If the proof was already reserved or spent, the PATCH will succeed (HTTP 204)
            // but no rows will be updated. We check if the proof is actually reserved.
            let reserved_proofs = self.get_reserved_proofs(operation_id).await?;
            if !reserved_proofs.iter().any(|p| p.y == *y) {
                return Err(DatabaseError::ProofNotUnspent);
            }
        }
        Ok(())
    }

    async fn release_proofs(&self, operation_id: &uuid::Uuid) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();

        // Update all proofs reserved by this operation back to Unspent
        let update = serde_json::json!({
            "state": State::Unspent.to_string(),
            "used_by_operation": null,
        });
        let path = format!(
            "rest/v1/proof?used_by_operation=eq.{}",
            url_encode(&op_id_str)
        );
        let (status, response_text) = self.patch_request(&path, &update).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "release_proofs failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<ProofInfo>, DatabaseError> {
        let op_id_str = operation_id.to_string();
        let path = format!(
            "rest/v1/proof?used_by_operation=eq.{}",
            url_encode(&op_id_str)
        );
        let (status, text) = self.get_request(&path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_reserved_proofs failed: HTTP {}",
                status
            )));
        }

        if let Some(items) = Self::parse_response::<ProofTable>(&text)? {
            let mut proofs = Vec::with_capacity(items.len());
            for mut p in items {
                self.decrypt_proof_table(&mut p).await;
                proofs.push(p.try_into()?);
            }
            Ok(proofs)
        } else {
            Ok(Vec::new())
        }
    }

    // ========== Quote reservation methods ==========

    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();

        let update = serde_json::json!({
            "used_by_operation": op_id_str,
        });

        // Use PostgREST filters on PATCH for atomic reservation.
        // We filter for both the quote ID and ensuring it is currently not reserved (used_by_operation IS NULL).
        let patch_path = format!(
            "rest/v1/melt_quote?id=eq.{}&used_by_operation=is.null",
            url_encode(quote_id)
        );

        let (status, response_text) = self.patch_request(&patch_path, &update).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "reserve_melt_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Verify that the quote was actually updated by checking if it's reserved for this operation.
        let quote = self.get_melt_quote(quote_id).await?;
        match quote {
            Some(q) => {
                if q.used_by_operation.as_deref() == Some(&op_id_str) {
                    Ok(())
                } else {
                    // Quote exists but was not reserved for us (already reserved by another operation).
                    Err(DatabaseError::QuoteAlreadyInUse)
                }
            }
            None => Err(DatabaseError::UnknownQuote),
        }
    }

    async fn release_melt_quote(&self, operation_id: &uuid::Uuid) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();

        let update = serde_json::json!({
            "used_by_operation": null,
        });
        let path = format!(
            "rest/v1/melt_quote?used_by_operation=eq.{}",
            url_encode(&op_id_str)
        );
        let (status, response_text) = self.patch_request(&path, &update).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "release_melt_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();

        let update = serde_json::json!({
            "used_by_operation": op_id_str,
        });

        // Use PostgREST filters on PATCH for atomic reservation.
        // We filter for both the quote ID and ensuring it is currently not reserved (used_by_operation IS NULL).
        let patch_path = format!(
            "rest/v1/mint_quote?id=eq.{}&used_by_operation=is.null",
            url_encode(quote_id)
        );

        let (status, response_text) = self.patch_request(&patch_path, &update).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "reserve_mint_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }

        // Verify that the quote was actually updated by checking if it's reserved for this operation.
        let quote = self.get_mint_quote(quote_id).await?;
        match quote {
            Some(q) => {
                if q.used_by_operation.as_deref() == Some(&op_id_str) {
                    Ok(())
                } else {
                    // Quote exists but was not reserved for us (already reserved by another operation).
                    Err(DatabaseError::QuoteAlreadyInUse)
                }
            }
            None => Err(DatabaseError::UnknownQuote),
        }
    }

    async fn release_mint_quote(&self, operation_id: &uuid::Uuid) -> Result<(), DatabaseError> {
        let op_id_str = operation_id.to_string();

        let update = serde_json::json!({
            "used_by_operation": null,
        });
        let path = format!(
            "rest/v1/mint_quote?used_by_operation=eq.{}",
            url_encode(&op_id_str)
        );
        let (status, response_text) = self.patch_request(&path, &update).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "release_mint_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn add_p2pk_key(
        &self,
        pubkey: &PublicKey,
        derivation_path: DerivationPath,
        derivation_index: u32,
    ) -> Result<(), DatabaseError> {
        let created_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| DatabaseError::Internal(format!("SystemTime error: {}", e)))?
            .as_secs();

        let item = P2PKSigningKeyTable {
            pubkey: hex::encode(pubkey.to_bytes()),
            derivation_index: derivation_index as i64,
            derivation_path: derivation_path.to_string(),
            created_time: created_time as i64,
            _extra: Default::default(),
        };

        let (status, response_text) = self
            .post_request(
                "rest/v1/p2pk_signing_key?on_conflict=pubkey,wallet_id",
                &item,
            )
            .await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_p2pk_key failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
    }

    async fn get_p2pk_key(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Option<wallet::P2PKSigningKey>, DatabaseError> {
        let path = format!(
            "rest/v1/p2pk_signing_key?pubkey=eq.{}",
            url_encode(&hex::encode(pubkey.to_bytes()))
        );
        let (status, text) = self.get_request(&path).await?;

        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "get_p2pk_key failed: HTTP {}",
                status
            )));
        }

        if let Some(rows) = Self::parse_response::<P2PKSigningKeyTable>(&text)? {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(row.try_into()?));
            }
        }
        Ok(None)
    }

    async fn list_p2pk_keys(&self) -> Result<Vec<wallet::P2PKSigningKey>, DatabaseError> {
        let path = "rest/v1/p2pk_signing_key?order=derivation_index.desc";
        let (status, text) = self.get_request(path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "list_p2pk_keys failed: HTTP {}",
                status
            )));
        }

        if let Some(rows) = Self::parse_response::<P2PKSigningKeyTable>(&text)? {
            rows.into_iter()
                .map(|row| row.try_into())
                .collect::<Result<Vec<_>, _>>()
        } else {
            Ok(Vec::new())
        }
    }

    async fn latest_p2pk(&self) -> Result<Option<wallet::P2PKSigningKey>, DatabaseError> {
        let path = "rest/v1/p2pk_signing_key?order=derivation_index.desc&limit=1";
        let (status, text) = self.get_request(path).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "latest_p2pk failed: HTTP {}",
                status
            )));
        }

        if let Some(rows) = Self::parse_response::<P2PKSigningKeyTable>(&text)? {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(row.try_into()?));
            }
        }
        Ok(None)
    }
}

// Data Structures for Supabase Tables (Serde)

// Note: All table structs use `deny_unknown_fields = false` (serde default) to allow
// extra columns added by other applications (e.g., user_id, opt_version) without breaking.

#[derive(Debug, Serialize, Deserialize)]
struct KVStoreTable {
    primary_namespace: String,
    secondary_namespace: String,
    key: String,
    value: String, // hex encoded bytea
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct MintTable {
    mint_url: String,
    name: Option<String>,
    pubkey: Option<String>,
    version: Option<String>,
    description: Option<String>,
    description_long: Option<String>,
    contact: Option<String>,
    nuts: Option<String>,
    icon_url: Option<String>,
    urls: Option<String>,
    motd: Option<String>,
    mint_time: Option<i64>,
    tos_url: Option<String>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl MintTable {
    fn from_info(mint_url: MintUrl, info: MintInfo) -> Result<Self, DatabaseError> {
        Ok(Self {
            mint_url: mint_url.to_string(),
            name: info.name,
            pubkey: info.pubkey.map(|p| hex::encode(p.to_bytes())),
            version: info
                .version
                .map(|v| serde_json::to_string(&v))
                .transpose()?,
            description: info.description,
            description_long: info.description_long,
            contact: info
                .contact
                .map(|c| serde_json::to_string(&c))
                .transpose()?,
            nuts: Some(serde_json::to_string(&info.nuts)?),
            icon_url: info.icon_url,
            urls: info.urls.map(|u| serde_json::to_string(&u)).transpose()?,
            motd: info.motd,
            mint_time: info.time.map(|t| t as i64),
            tos_url: info.tos_url,
            _extra: Default::default(),
        })
    }
}

impl TryInto<MintInfo> for MintTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<MintInfo, Self::Error> {
        // Helper to filter empty strings before JSON parsing
        fn parse_json_field<T: serde::de::DeserializeOwned>(
            field: Option<String>,
        ) -> Result<Option<T>, serde_json::Error> {
            match field {
                Some(s) if !s.trim().is_empty() => {
                    let s = s.trim();
                    match serde_json::from_str::<T>(s) {
                        Ok(v) => Ok(Some(v)),
                        Err(e) => {
                            // If it fails to parse, try wrapping it in quotes in case it's a bare string
                            // but only if it doesn't already look like a JSON object or array
                            if !s.starts_with('{') && !s.starts_with('[') && !s.starts_with('"') {
                                let quoted = format!("\"{}\"", s);
                                if let Ok(v) = serde_json::from_str::<T>(&quoted) {
                                    return Ok(Some(v));
                                }
                            }
                            Err(e)
                        }
                    }
                }
                _ => Ok(None),
            }
        }

        Ok(MintInfo {
            name: self.name,
            pubkey: self
                .pubkey
                .map(|p| {
                    PublicKey::from_hex(&p)
                        .map_err(|_| DatabaseError::Internal("Invalid pubkey hex".into()))
                })
                .transpose()?,
            version: parse_json_field(self.version)?,
            description: self.description,
            description_long: self.description_long,
            contact: parse_json_field(self.contact)?,
            nuts: parse_json_field(self.nuts)?.unwrap_or_default(),
            icon_url: self.icon_url,
            urls: parse_json_field(self.urls)?,
            motd: self.motd,
            time: self.mint_time.map(|t| t as u64),
            tos_url: self.tos_url,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KeySetTable {
    mint_url: String,
    id: String,
    unit: String,
    active: bool,
    input_fee_ppk: i64,
    final_expiry: Option<i64>,
    keyset_u32: Option<i64>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl KeySetTable {
    fn from_info(mint_url: MintUrl, info: KeySetInfo) -> Result<Self, DatabaseError> {
        Ok(Self {
            mint_url: mint_url.to_string(),
            id: info.id.to_string(),
            unit: info.unit.to_string(),
            active: info.active,
            input_fee_ppk: info.input_fee_ppk as i64,
            final_expiry: info.final_expiry.map(|v| v as i64),
            keyset_u32: Some(u32::from(info.id) as i64),
            _extra: Default::default(),
        })
    }
}

impl TryInto<KeySetInfo> for KeySetTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<KeySetInfo, Self::Error> {
        Ok(KeySetInfo {
            id: Id::from_str(&self.id).map_err(|_| DatabaseError::InvalidKeysetId)?,
            unit: CurrencyUnit::from_str(&self.unit)
                .map_err(|_| DatabaseError::Internal("Invalid unit".into()))?,
            active: self.active,
            input_fee_ppk: self.input_fee_ppk as u64,
            final_expiry: self.final_expiry.map(|v| v as u64),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct P2PKSigningKeyTable {
    pubkey: String,
    derivation_index: i64,
    derivation_path: String,
    created_time: i64,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<wallet::P2PKSigningKey> for P2PKSigningKeyTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<wallet::P2PKSigningKey, Self::Error> {
        Ok(wallet::P2PKSigningKey {
            pubkey: PublicKey::from_hex(&self.pubkey)
                .map_err(|_| DatabaseError::Internal("Invalid pubkey hex".into()))?,
            derivation_path: DerivationPath::from_str(&self.derivation_path)
                .map_err(|_| DatabaseError::Internal("Invalid derivation path".into()))?,
            derivation_index: self.derivation_index as u32,
            created_time: self.created_time as u64,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KeyTable {
    id: String,
    keys: String, // json string
    keyset_u32: Option<i64>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl KeyTable {
    fn from_keyset(keyset: &KeySet) -> Result<Self, DatabaseError> {
        Ok(Self {
            id: keyset.id.to_string(),
            keys: serde_json::to_string(&keyset.keys)?,
            keyset_u32: Some(u32::from(keyset.id) as i64),
            _extra: Default::default(),
        })
    }
}

impl TryInto<Keys> for KeyTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<Keys, Self::Error> {
        Ok(serde_json::from_str(&self.keys)?)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MintQuoteTable {
    id: String,
    mint_url: String,
    amount: i64,
    unit: String,
    request: Option<String>,
    state: String,
    expiry: i64,
    secret_key: Option<String>,
    payment_method: String,
    amount_issued: i64,
    amount_paid: i64,
    #[serde(default)]
    used_by_operation: Option<String>,
    #[serde(default)]
    version: Option<i32>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<MintQuote> for MintQuoteTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<MintQuote, Self::Error> {
        Ok(MintQuote {
            id: self.id,
            mint_url: MintUrl::from_str(&self.mint_url)
                .map_err(|e| DatabaseError::Internal(e.to_string()))?,
            amount: Some(cdk_common::Amount::from(self.amount as u64)),
            unit: CurrencyUnit::from_str(&self.unit)
                .map_err(|_| DatabaseError::Internal("Invalid unit".into()))?,
            request: self
                .request
                .ok_or(DatabaseError::Internal("Missing request".into()))?,
            state: cdk_common::nuts::MintQuoteState::from_str(&self.state)
                .map_err(|_| DatabaseError::Internal("Invalid state".into()))?,
            expiry: self.expiry as u64,
            secret_key: self
                .secret_key
                .map(|k| cdk_common::nuts::SecretKey::from_str(&k))
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid secret key".into()))?,
            payment_method: cdk_common::PaymentMethod::from_str(&self.payment_method)
                .map_err(|_| DatabaseError::Internal("Invalid payment method".into()))?,
            amount_issued: cdk_common::Amount::from(self.amount_issued as u64),
            amount_paid: cdk_common::Amount::from(self.amount_paid as u64),
            used_by_operation: self.used_by_operation,
            version: self.version.unwrap_or(0) as u32,
        })
    }
}

impl TryFrom<MintQuote> for MintQuoteTable {
    type Error = DatabaseError;
    fn try_from(q: MintQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            id: q.id,
            mint_url: q.mint_url.to_string(),
            amount: q.amount.map(|a| a.to_u64() as i64).unwrap_or(0),
            unit: q.unit.to_string(),
            request: Some(q.request),
            state: q.state.to_string(),
            expiry: q.expiry as i64,
            secret_key: q.secret_key.map(|k| k.to_string()),
            payment_method: q.payment_method.to_string(),
            amount_issued: q.amount_issued.to_u64() as i64,
            amount_paid: q.amount_paid.to_u64() as i64,
            used_by_operation: q.used_by_operation,
            version: Some(q.version as i32),
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MeltQuoteTable {
    id: String,
    unit: String,
    amount: i64,
    request: String,
    fee_reserve: i64,
    state: String,
    expiry: i64,
    payment_preimage: Option<String>,
    payment_method: String,
    #[serde(default)]
    mint_url: Option<String>,
    #[serde(default)]
    used_by_operation: Option<String>,
    #[serde(default)]
    version: Option<i32>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<wallet::MeltQuote> for MeltQuoteTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<wallet::MeltQuote, Self::Error> {
        Ok(wallet::MeltQuote {
            id: self.id,
            mint_url: self
                .mint_url
                .as_deref()
                .map(cdk_common::mint_url::MintUrl::from_str)
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid mint URL".into()))?,
            unit: CurrencyUnit::from_str(&self.unit)
                .map_err(|_| DatabaseError::Internal("Invalid unit".into()))?,
            amount: cdk_common::Amount::from(self.amount as u64),
            request: self.request,
            fee_reserve: cdk_common::Amount::from(self.fee_reserve as u64),
            state: cdk_common::nuts::MeltQuoteState::from_str(&self.state)
                .map_err(|_| DatabaseError::Internal("Invalid state".into()))?,
            expiry: self.expiry as u64,
            payment_preimage: self.payment_preimage,
            payment_method: cdk_common::PaymentMethod::from_str(&self.payment_method)
                .map_err(|_| DatabaseError::Internal("Invalid payment method".into()))?,
            used_by_operation: self.used_by_operation,
            version: self.version.unwrap_or(0) as u32,
        })
    }
}

impl TryFrom<wallet::MeltQuote> for MeltQuoteTable {
    type Error = DatabaseError;
    fn try_from(q: wallet::MeltQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            id: q.id,
            mint_url: q.mint_url.map(|u| u.to_string()),
            unit: q.unit.to_string(),
            amount: q.amount.to_u64() as i64,
            request: q.request,
            fee_reserve: q.fee_reserve.to_u64() as i64,
            state: q.state.to_string(),
            expiry: q.expiry as i64,
            payment_preimage: q.payment_preimage,
            payment_method: q.payment_method.to_string(),
            used_by_operation: q.used_by_operation,
            version: Some(q.version as i32),
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProofTable {
    y: String,
    mint_url: String,
    state: String,
    spending_condition: Option<String>,
    unit: String,
    amount: i64,
    keyset_id: String,
    secret: String,
    c: String,
    witness: Option<String>,
    dleq_e: Option<String>,
    dleq_s: Option<String>,
    dleq_r: Option<String>,
    #[serde(default)]
    used_by_operation: Option<String>,
    #[serde(default)]
    created_by_operation: Option<String>,
    #[serde(default)]
    p2pk_e: Option<String>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<ProofInfo> for ProofTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<ProofInfo, Self::Error> {
        let y = PublicKey::from_hex(&self.y)
            .map_err(|_| DatabaseError::Internal("Invalid y".into()))?;
        let c = PublicKey::from_hex(&self.c)
            .map_err(|_| DatabaseError::Internal("Invalid c".into()))?;
        Ok(ProofInfo {
            y,
            mint_url: MintUrl::from_str(&self.mint_url)
                .map_err(|e| DatabaseError::Internal(e.to_string()))?,
            state: cdk_common::nuts::State::from_str(&self.state)
                .map_err(|_| DatabaseError::Internal("Invalid state".into()))?,
            spending_condition: self
                .spending_condition
                .filter(|s| !s.trim().is_empty())
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
            unit: CurrencyUnit::from_str(&self.unit)
                .map_err(|_| DatabaseError::Internal("Invalid unit".into()))?,
            proof: cdk_common::Proof {
                amount: cdk_common::Amount::from(self.amount as u64),
                keyset_id: Id::from_str(&self.keyset_id)
                    .map_err(|_| DatabaseError::InvalidKeysetId)?,
                secret: Secret::from_str(&self.secret)
                    .map_err(|_| DatabaseError::Internal("Invalid secret".into()))?,
                c,
                witness: self
                    .witness
                    .filter(|w| !w.trim().is_empty())
                    .map(|w| serde_json::from_str(&w))
                    .transpose()?,
                dleq: match (self.dleq_e, self.dleq_s, self.dleq_r) {
                    (Some(e), Some(s), Some(r)) => Some(cdk_common::ProofDleq {
                        e: cdk_common::SecretKey::from_hex(&e)
                            .map_err(|_| DatabaseError::Internal("Invalid dleq_e".into()))?,
                        s: cdk_common::SecretKey::from_hex(&s)
                            .map_err(|_| DatabaseError::Internal("Invalid dleq_s".into()))?,
                        r: cdk_common::SecretKey::from_hex(&r)
                            .map_err(|_| DatabaseError::Internal("Invalid dleq_r".into()))?,
                    }),
                    _ => None,
                },
                p2pk_e: self
                    .p2pk_e
                    .map(|s| PublicKey::from_hex(&s))
                    .transpose()
                    .map_err(|_| DatabaseError::Internal("Invalid p2pk_e".into()))?,
            },
            used_by_operation: self
                .used_by_operation
                .map(|s| uuid::Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid used_by_operation uuid".into()))?,
            created_by_operation: self
                .created_by_operation
                .map(|s| uuid::Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid created_by_operation uuid".into()))?,
        })
    }
}

impl TryFrom<ProofInfo> for ProofTable {
    type Error = DatabaseError;
    fn try_from(p: ProofInfo) -> Result<Self, Self::Error> {
        Ok(Self {
            y: hex::encode(p.y.to_bytes()),
            mint_url: p.mint_url.to_string(),
            state: p.state.to_string(),
            spending_condition: p
                .spending_condition
                .map(|s| serde_json::to_string(&s))
                .transpose()?,
            unit: p.unit.to_string(),
            amount: p.proof.amount.to_u64() as i64,
            keyset_id: p.proof.keyset_id.to_string(),
            secret: p.proof.secret.to_string(),
            c: hex::encode(p.proof.c.to_bytes()),
            witness: p
                .proof
                .witness
                .map(|w| serde_json::to_string(&w))
                .transpose()?,
            dleq_e: p
                .proof
                .dleq
                .as_ref()
                .map(|d| hex::encode(d.e.to_secret_bytes())),
            dleq_s: p
                .proof
                .dleq
                .as_ref()
                .map(|d| hex::encode(d.s.to_secret_bytes())),
            dleq_r: p
                .proof
                .dleq
                .as_ref()
                .map(|d| hex::encode(d.r.to_secret_bytes())),
            used_by_operation: p.used_by_operation.map(|u| u.to_string()),
            created_by_operation: p.created_by_operation.map(|u| u.to_string()),
            p2pk_e: p.proof.p2pk_e.map(|e| hex::encode(e.to_bytes())),
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TransactionTable {
    id: String,
    mint_url: String,
    direction: String,
    unit: String,
    amount: i64,
    fee: i64,
    ys: Option<Vec<String>>,
    timestamp: i64,
    memo: Option<String>,
    metadata: Option<String>,
    quote_id: Option<String>,
    payment_request: Option<String>,
    payment_proof: Option<String>,
    payment_method: Option<String>,
    #[serde(default)]
    saga_id: Option<String>,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<Transaction> for TransactionTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<Transaction, Self::Error> {
        let id_bytes = hex::decode(&self.id)
            .map_err(|_| DatabaseError::Internal("Invalid transaction id hex".into()))?;
        let _id_arr: [u8; 32] = id_bytes
            .try_into()
            .map_err(|_| DatabaseError::Internal("Invalid transaction id len".into()))?;

        let ys = match self.ys {
            Some(strs) => strs
                .into_iter()
                .map(|s| {
                    PublicKey::from_hex(&s)
                        .map_err(|_| DatabaseError::Internal("Invalid y hex".into()))
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => vec![],
        };

        Ok(Transaction {
            mint_url: MintUrl::from_str(&self.mint_url)
                .map_err(|e| DatabaseError::Internal(e.to_string()))?,
            direction: TransactionDirection::from_str(&self.direction)
                .map_err(|_| DatabaseError::Internal("Invalid direction".into()))?,
            unit: CurrencyUnit::from_str(&self.unit)
                .map_err(|_| DatabaseError::Internal("Invalid unit".into()))?,
            amount: cdk_common::Amount::from(self.amount as u64),
            fee: cdk_common::Amount::from(self.fee as u64),
            ys,
            timestamp: self.timestamp as u64,
            memo: self.memo,
            metadata: self
                .metadata
                .filter(|m| !m.trim().is_empty())
                .map(|m| serde_json::from_str(&m))
                .transpose()?
                .unwrap_or_default(),
            quote_id: self.quote_id,
            payment_request: self.payment_request,
            payment_proof: self.payment_proof,
            payment_method: self
                .payment_method
                .map(|p| cdk_common::PaymentMethod::from_str(&p))
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid payment method".into()))?,
            saga_id: self
                .saga_id
                .map(|s| uuid::Uuid::parse_str(&s))
                .transpose()
                .map_err(|_| DatabaseError::Internal("Invalid saga_id uuid".into()))?,
        })
    }
}

impl TryFrom<Transaction> for TransactionTable {
    type Error = DatabaseError;
    fn try_from(t: Transaction) -> Result<Self, Self::Error> {
        Ok(Self {
            id: t.id().to_string(),
            mint_url: t.mint_url.to_string(),
            direction: t.direction.to_string(),
            unit: t.unit.to_string(),
            amount: t.amount.to_u64() as i64,
            fee: t.fee.to_u64() as i64,
            ys: Some(t.ys.iter().map(|y| hex::encode(y.to_bytes())).collect()),
            timestamp: t.timestamp as i64,
            memo: t.memo,
            metadata: if t.metadata.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&t.metadata)?)
            },
            quote_id: t.quote_id,
            payment_request: t.payment_request,
            payment_proof: t.payment_proof,
            payment_method: t.payment_method.map(|p| p.to_string()),
            saga_id: t.saga_id.map(|u| u.to_string()),
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SagaTable {
    id: String,
    data: String, // JSON-serialized WalletSaga
    version: i32,
    completed: bool,
    created_at: i64,
    updated_at: i64,
    /// Extra fields from other applications
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

/// Response from Supabase Auth sign-up/sign-in
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SupabaseAuthResponse {
    /// Access token
    pub access_token: String,
    /// Token type
    pub token_type: String,
    /// Expires in
    pub expires_in: Option<i64>,
    /// Refresh token
    pub refresh_token: Option<String>,
    /// User
    pub user: serde_json::Value,
}

/// Helper for Supabase Authentication
#[derive(Debug)]
pub struct SupabaseAuth;

impl SupabaseAuth {
    /// Sign up a new user with email and password
    pub async fn signup(
        url: &Url,
        api_key: &str,
        email: &str,
        password: &str,
    ) -> Result<SupabaseAuthResponse, Error> {
        let auth_url = url
            .join("auth/v1/signup")
            .map_err(|e| Error::Supabase(format!("Invalid auth URL: {}", e)))?;

        let client = Client::new();
        let body = serde_json::json!({
            "email": email,
            "password": password
        });

        let response = client
            .post(auth_url)
            .header("apikey", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Supabase(format!(
                "Supabase signup failed: HTTP {} - {}",
                status, text
            )));
        }

        response.json().await.map_err(Error::Reqwest)
    }

    /// Sign in a user with email and password
    pub async fn signin(
        url: &Url,
        api_key: &str,
        email: &str,
        password: &str,
    ) -> Result<SupabaseAuthResponse, Error> {
        let auth_url = url
            .join("auth/v1/token?grant_type=password")
            .map_err(|e| Error::Supabase(format!("Invalid auth URL: {}", e)))?;

        let client = Client::new();
        let body = serde_json::json!({
            "email": email,
            "password": password
        });

        let response = client
            .post(auth_url)
            .header("apikey", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Supabase(format!(
                "Supabase signin failed: HTTP {} - {}",
                status, text
            )));
        }

        response.json().await.map_err(Error::Reqwest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_encryption_password_key_derivation() {
        // SHA-256("password") == 5e884898...
        let key = sha256::Hash::hash(b"password");
        assert_eq!(
            hex::encode(key.as_byte_array()),
            "5e884898da28047151d0e56f8dc6292773603d0d6aabbdd62a11ef721d1542d8"
        );
    }
}
