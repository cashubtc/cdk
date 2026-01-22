use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cdk_common::auth::oidc::OidcClient;
use cdk_common::common::ProofInfo;
use cdk_common::database::wallet::Database;
use cdk_common::database::{Error as DatabaseError, KVStoreDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{
    CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use cdk_sql_common::database::DatabaseExecutor;
use cdk_sql_common::stmt::{Column, Statement};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use url::Url;

use crate::Error;

#[rustfmt::skip]
mod migrations {
    include!(concat!(env!("OUT_DIR"), "/migrations_supabase.rs"));
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

    use base64::engine::general_purpose;
    use base64::Engine as _;

    let decoded = general_purpose::URL_SAFE_NO_PAD.decode(payload_part).ok()?;

    #[derive(Deserialize)]
    struct Claims {
        exp: Option<u64>,
    }

    let claims: Claims = serde_json::from_slice(&decoded).ok()?;
    claims.exp
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
#[derive(Debug, Clone)]
pub struct SupabaseWalletDatabase {
    url: Url,
    api_key: String,
    jwt_token: Arc<RwLock<Option<String>>>,
    refresh_token: Arc<RwLock<Option<String>>>,
    token_expiration: Arc<RwLock<Option<u64>>>,
    oidc_client: Arc<RwLock<Option<OidcClient>>>,
    client: Client,
}

impl SupabaseWalletDatabase {
    /// Create a new SupabaseWalletDatabase with API key only (legacy behavior)
    ///
    /// This will automatically run migrations.
    pub async fn new(url: Url, api_key: String) -> Result<Self, Error> {
        let db = Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            oidc_client: Arc::new(RwLock::new(None)),
            client: Client::new(),
        };

        db.migrate().await?;

        Ok(db)
    }

    /// Create a new SupabaseWalletDatabase with OIDC client for auth
    ///
    /// This will automatically run migrations.
    pub async fn with_oidc(
        url: Url,
        api_key: String,
        oidc_client: OidcClient,
    ) -> Result<Self, Error> {
        let db = Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            oidc_client: Arc::new(RwLock::new(Some(oidc_client))),
            client: Client::new(),
        };

        db.migrate().await?;

        Ok(db)
    }

    /// Run database migrations
    pub async fn migrate(&self) -> Result<(), Error> {
        // We use cdk_sql_common::migrate but we need to implement DatabaseExecutor
        // for SupabaseWalletDatabase which uses the exec_sql RPC.
        match cdk_sql_common::migrate(self, "supabase", migrations::MIGRATIONS).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // If it fails because the exec_sql function doesn't exist, we should give a helpful error
                let err_str = e.to_string();
                if err_str.contains("404") || err_str.contains("exec_sql") {
                    tracing::error!("Supabase migrations failed: exec_sql RPC function not found. You must run 001_initial_schema.sql manually once.");
                }
                Err(e.into())
            }
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

    /// Refresh the access token using the stored refresh token
    pub async fn refresh_access_token(&self) -> Result<(), Error> {
        let oidc_client = self.oidc_client.read().await;

        if let Some(oidc) = oidc_client.as_ref() {
            let refresh_token = self.refresh_token.read().await.clone();

            if let Some(refresh) = refresh_token {
                if let Some(client_id) = oidc.client_id() {
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

                    return Ok(());
                } else {
                    return Err(Error::Supabase(
                        "Client ID not set in OIDC client".to_string(),
                    ));
                }
            }
        }

        Err(Error::Supabase(
            "No OIDC client or refresh token available".to_string(),
        ))
    }

    /// Get the current JWT token if set
    pub async fn get_jwt_token(&self) -> Option<String> {
        self.jwt_token.read().await.clone()
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

        tracing::debug!(method = "GET", url = %url, status = %status, response = %text, "Supabase response");

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

        tracing::debug!(method = "POST", url = %url, body = ?body, "Supabase request");

        let res = self
            .client
            .post(url.clone())
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .header("Prefer", "resolution=merge-duplicates")
            .json(body)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        let text = res.text().await.map_err(Error::Reqwest)?;

        tracing::debug!(method = "POST", url = %url, status = %status, response = %text, "Supabase response");

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

        tracing::debug!(method = "PATCH", url = %url, body = ?body, "Supabase request");

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

        tracing::debug!(method = "PATCH", url = %url, status = %status, response = %text, "Supabase response");

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

        tracing::debug!(method = "DELETE", url = %url, status = %status, response = %text, "Supabase response");

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
impl DatabaseExecutor for SupabaseWalletDatabase {
    fn name() -> &'static str {
        "supabase"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, DatabaseError> {
        let (sql, params) = statement.to_sql()?;

        // Special case for migrations table interactions to avoid needing a complex exec_sql with params
        if sql.contains("INSERT INTO migrations") {
            let name = params
                .first()
                .and_then(|v| match v {
                    cdk_sql_common::value::Value::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            let body = serde_json::json!({ "name": name });
            let (status, text) = self.post_request("rest/v1/migrations", &body).await?;
            if !status.is_success() {
                return Err(DatabaseError::Database(Box::new(std::io::Error::other(
                    format!("Failed to insert migration: {} - {}", status, text),
                ))));
            }
            return Ok(1);
        }

        // For everything else, use exec_sql RPC
        let body = serde_json::json!({ "query": sql });
        let (status, text) = self.post_request("rest/v1/rpc/exec_sql", &body).await?;

        if !status.is_success() {
            return Err(DatabaseError::Database(Box::new(std::io::Error::other(
                format!("Supabase RPC exec_sql failed: {} - {}", status, text),
            ))));
        }

        Ok(0)
    }

    async fn fetch_one(&self, _statement: Statement) -> Result<Option<Vec<Column>>, DatabaseError> {
        Err(DatabaseError::Database(Box::new(std::io::Error::other(
            "fetch_one not implemented for Supabase executor",
        ))))
    }

    async fn fetch_all(&self, _statement: Statement) -> Result<Vec<Vec<Column>>, DatabaseError> {
        Err(DatabaseError::Database(Box::new(std::io::Error::other(
            "fetch_all not implemented for Supabase executor",
        ))))
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, DatabaseError> {
        let (sql, params) = statement.to_sql()?;

        // Special case for checking migrations
        if sql.contains("SELECT name FROM migrations") {
            let name = params
                .first()
                .and_then(|v| match v {
                    cdk_sql_common::value::Value::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            let path = format!("rest/v1/migrations?name=eq.{}", url_encode(&name));
            let (status, text) = self.get_request(&path).await?;

            if status.is_success() {
                if let Ok(Some(items)) = Self::parse_response::<serde_json::Value>(&text) {
                    if !items.is_empty() {
                        return Ok(Some(Column::Text(name)));
                    }
                }
            }
            return Ok(None);
        }

        Err(DatabaseError::Database(Box::new(std::io::Error::other(
            "pluck not implemented for Supabase executor",
        ))))
    }

    async fn batch(&self, statement: Statement) -> Result<(), DatabaseError> {
        self.execute(statement).await.map(|_| ())
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
                return Ok(Some(bytes));
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

        if !status.is_success() {
            return Ok(None);
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
            for p in proofs {
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
            proofs.into_iter().map(|p| p.try_into()).collect()
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

        if !status.is_success() {
            return Ok(None);
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
        // Add new proofs
        if !added.is_empty() {
            let items: Result<Vec<ProofTable>, DatabaseError> =
                added.into_iter().map(|p| p.try_into()).collect();
            let items = items?;

            let (status, response_text) = self
                .post_request("rest/v1/proof?on_conflict=y", &items)
                .await?;

            if !status.is_success() {
                return Err(DatabaseError::Internal(format!(
                    "update_proofs (add) failed: HTTP {} - {}",
                    status, response_text
                )));
            }
        }

        // Remove proofs by y values
        if !removed_ys.is_empty() {
            let ys_str: Vec<String> = removed_ys
                .iter()
                .map(|y| hex::encode(y.to_bytes()))
                .collect();
            let filter = format!("({})", ys_str.join(","));
            let path = format!("rest/v1/proof?y=in.{}", filter);

            let (status, response_text) = self.delete_request(&path).await?;

            if !status.is_success() {
                return Err(DatabaseError::Internal(format!(
                    "update_proofs (remove) failed: HTTP {} - {}",
                    status, response_text
                )));
            }
        }

        Ok(())
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
        let (status, response_text) = self.post_request("rest/v1/transactions", &item).await?;

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

        tracing::debug!(method = "POST", url = %url, body = ?rpc_body, "Supabase RPC request");

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

        tracing::debug!(method = "POST", url = %url, status = %status, response = %text, "Supabase RPC response");

        if status.is_success() {
            // RPC returns the new counter value directly
            let new_counter: i32 = serde_json::from_str(&text).map_err(|e| {
                DatabaseError::Internal(format!(
                    "Failed to parse counter response '{}': {}",
                    text, e
                ))
            })?;
            return Ok(new_counter as u32);
        }

        // If RPC fails (e.g., function doesn't exist), fall back to upsert approach
        // This provides backwards compatibility with databases that haven't run migration 002
        tracing::warn!(
            "RPC increment_keyset_counter failed (HTTP {}), falling back to upsert: {}",
            status,
            text
        );

        // Fallback: Use upsert with on_conflict
        // Note: This is not perfectly atomic but better than DELETE + INSERT
        let path = format!(
            "rest/v1/keyset_counter?keyset_id=eq.{}",
            url_encode(&keyset_id.to_string())
        );
        let (get_status, get_text) = self.get_request(&path).await?;

        let current = if get_status.is_success() {
            if let Some(items) = Self::parse_response::<KeysetCounterTable>(&get_text)? {
                items.into_iter().next().map(|i| i.counter).unwrap_or(0)
            } else {
                0
            }
        } else {
            0
        };

        let new = current + count;

        // Use upsert (POST with on_conflict)
        let item = KeysetCounterTable {
            keyset_id: keyset_id.to_string(),
            counter: new,
            _extra: Default::default(),
        };

        let (upsert_status, response_text) = self
            .post_request("rest/v1/keyset_counter?on_conflict=keyset_id", &item)
            .await?;

        if !upsert_status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "increment_keyset_counter failed: HTTP {} - {}",
                upsert_status, response_text
            )));
        }

        Ok(new)
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

        let (status, response_text) = self.post_request("rest/v1/mint", &info_table).await?;

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
            .post_request("rest/v1/keyset?on_conflict=id", &items)
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
        let item: MintQuoteTable = quote.try_into()?;
        let (status, response_text) = self.post_request("rest/v1/mint_quote", &item).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_mint_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
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
        let item: MeltQuoteTable = quote.try_into()?;
        let (status, response_text) = self.post_request("rest/v1/melt_quote", &item).await?;

        if !status.is_success() {
            return Err(DatabaseError::Internal(format!(
                "add_melt_quote failed: HTTP {} - {}",
                status, response_text
            )));
        }
        Ok(())
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

        let (status, response_text) = self.post_request("rest/v1/key", &item).await?;

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
        let item = KVStoreTable {
            primary_namespace: primary_namespace.to_string(),
            secondary_namespace: secondary_namespace.to_string(),
            key: key.to_string(),
            value: hex::encode(value),
            _extra: Default::default(),
        };

        let (status, response_text) = self.post_request("rest/v1/kv_store", &item).await?;

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
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl TryInto<wallet::MeltQuote> for MeltQuoteTable {
    type Error = DatabaseError;
    fn try_into(self) -> Result<wallet::MeltQuote, Self::Error> {
        Ok(wallet::MeltQuote {
            id: self.id,
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
        })
    }
}

impl TryFrom<wallet::MeltQuote> for MeltQuoteTable {
    type Error = DatabaseError;
    fn try_from(q: wallet::MeltQuote) -> Result<Self, Self::Error> {
        Ok(Self {
            id: q.id,
            unit: q.unit.to_string(),
            amount: q.amount.to_u64() as i64,
            request: q.request,
            fee_reserve: q.fee_reserve.to_u64() as i64,
            state: q.state.to_string(),
            expiry: q.expiry as i64,
            payment_preimage: q.payment_preimage,
            payment_method: q.payment_method.to_string(),
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
            },
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
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KeysetCounterTable {
    keyset_id: String,
    counter: u32,
    /// Extra fields from other applications (captured during deserialization, ignored during serialization)
    #[serde(default, skip_serializing, flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
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
            _extra: Default::default(),
        })
    }
}
