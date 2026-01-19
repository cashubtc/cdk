use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use cdk_common::auth::oidc::OidcClient;
use cdk_common::common::ProofInfo;
use cdk_common::database::{
    wallet::Database, DbTransactionFinalizer, Error as DatabaseError, KVStoreDatabase,
    KVStoreTransaction,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::nuts::{
    CurrencyUnit, Id, KeySet, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use cdk_common::secret::Secret;
use cdk_common::wallet::{self, MintQuote, Transaction, TransactionDirection, TransactionId};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use url::Url;

use crate::Error;

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
    pub fn new(url: Url, api_key: String) -> Self {
        Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            oidc_client: Arc::new(RwLock::new(None)),
            client: Client::new(),
        }
    }

    /// Create a new SupabaseWalletDatabase with OIDC client for auth
    pub fn with_oidc(url: Url, api_key: String, oidc_client: OidcClient) -> Self {
        Self {
            url,
            api_key,
            jwt_token: Arc::new(RwLock::new(None)),
            refresh_token: Arc::new(RwLock::new(None)),
            token_expiration: Arc::new(RwLock::new(None)),
            oidc_client: Arc::new(RwLock::new(Some(oidc_client))),
            client: Client::new(),
        }
    }

    /// Set or update the JWT token for authentication
    pub async fn set_jwt_token(&self, token: Option<String>) {
        let mut jwt = self.jwt_token.write().await;
        *jwt = token;
    }

    /// Set refresh token
    pub async fn set_refresh_token(&self, token: Option<String>) {
        let mut refresh = self.refresh_token.write().await;
        *refresh = token;
    }

    /// Set token expiration
    pub async fn set_token_expiration(&self, expiration: Option<u64>) {
        let mut exp = self.token_expiration.write().await;
        *exp = expiration;
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
                            .expect("SystemTime should be after UNIX_EPOCH")
                            .as_secs()
                            + expires_in as u64;
                        self.set_token_expiration(Some(expiration)).await;
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

    pub fn join_url(&self, path: &str) -> Result<Url, DatabaseError> {
        self.url
            .join(path)
            .map_err(|e| DatabaseError::Internal(e.to_string()))
    }

    pub async fn begin_db_transaction(
        &self,
    ) -> Result<Box<SupabaseWalletTransaction>, DatabaseError> {
        Ok(Box::new(SupabaseWalletTransaction {
            database: self.clone(),
        }))
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
        let url = self.join_url(&format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}&key=eq.{}",
            primary_namespace, secondary_namespace, key
        ))?;

        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let items: Vec<KVStoreTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            let bytes = hex::decode(item.value)
                .map_err(|_| DatabaseError::Internal("Invalid hex in kv_store".into()))?;
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err> {
        let url = self.join_url(&format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}",
            primary_namespace, secondary_namespace
        ))?;

        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let items: Vec<KVStoreTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        Ok(items.into_iter().map(|i| i.key).collect())
    }
}
#[async_trait]
impl Database<DatabaseError> for SupabaseWalletDatabase {
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, DatabaseError> {
        let url = self.join_url(&format!("rest/v1/mint?mint_url=eq.{}", mint_url))?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let mints: Vec<MintTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(mint) = mints.into_iter().next() {
            Ok(Some(mint.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, DatabaseError> {
        let url = self.join_url("rest/v1/mint")?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }

        let text = res.text().await.map_err(Error::Reqwest)?;

        if text.trim().is_empty() {
            return Ok(HashMap::new());
        }

        let mints: Vec<MintTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut map = HashMap::new();
        for mint in mints {
            map.insert(
                MintUrl::from_str(&mint.mint_url)
                    .map_err(|e| DatabaseError::Internal(e.to_string()))?,
                Some(mint.try_into()?),
            );
        }
        Ok(map)
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, DatabaseError> {
        let url = self.join_url(&format!("rest/v1/keyset?mint_url=eq.{}", mint_url))?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }

        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() {
            return Ok(None);
        }

        let keysets: Vec<KeySetTable> = serde_json::from_str(&text).map_err(Error::Serde)?;

        if keysets.is_empty() {
            return Ok(None);
        }

        let mut result = Vec::new();
        for ks in keysets {
            result.push(ks.try_into()?);
        }
        Ok(Some(result))
    }

    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.get_keyset_by_id(keyset_id).await
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.get_mint_quote(quote_id).await
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, DatabaseError> {
        let url = self.join_url("rest/v1/mint_quote")?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let quotes: Vec<MintQuoteTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for q in quotes {
            result.push(q.try_into()?);
        }
        Ok(result)
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, DatabaseError> {
        let url = self.join_url("rest/v1/mint_quote?amount_issued=eq.0")?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let quotes: Vec<MintQuoteTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for q in quotes {
            result.push(q.try_into()?);
        }
        Ok(result)
    }

    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.get_melt_quote(quote_id).await
    }

    async fn get_melt_quotes(&self) -> Result<Vec<wallet::MeltQuote>, DatabaseError> {
        let url = self.join_url("rest/v1/melt_quote")?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let quotes: Vec<MeltQuoteTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for q in quotes {
            result.push(q.try_into()?);
        }
        Ok(result)
    }

    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.get_keys(id).await
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.get_proofs(mint_url, unit, state, spending_conditions)
            .await
    }

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, DatabaseError> {
        let ys_str: Vec<String> = ys.iter().map(|y| hex::encode(y.to_bytes())).collect();
        let filter = format!("({},)", ys_str.join(","));

        let url = self.join_url(&format!("rest/v1/proof?y=in.{}", filter))?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let proofs: Vec<ProofTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for p in proofs {
            result.push(p.try_into()?);
        }
        Ok(result)
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
    ) -> Result<u64, DatabaseError> {
        let proofs = self.get_proofs(mint_url, unit, state, None).await?;
        Ok(proofs.iter().map(|p| p.proof.amount.to_u64()).sum())
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, DatabaseError> {
        let id_hex = transaction_id.to_string();
        let url = self.join_url(&format!("rest/v1/transactions?id=eq.\\x{}", id_hex))?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let txs: Vec<TransactionTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(t) = txs.into_iter().next() {
            Ok(Some(t.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, DatabaseError> {
        let mut query = String::from("rest/v1/transactions?select=*");
        if let Some(url) = mint_url {
            query.push_str(&format!("&mint_url=eq.{}", url));
        }
        if let Some(d) = direction {
            query.push_str(&format!("&direction=eq.{}", d));
        }
        if let Some(u) = unit {
            query.push_str(&format!("&unit=eq.{}", u));
        }

        let url = self.join_url(&query)?;
        let auth_bearer = self.get_auth_bearer().await;
        let res = self
            .client
            .get(url)
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", auth_bearer))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(Vec::new());
        }
        let txs: Vec<TransactionTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for t in txs {
            result.push(t.try_into()?);
        }
        Ok(result)
    }

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.update_proofs(added, removed_ys).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.update_proofs_state(ys, state).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_transaction(transaction).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.remove_transaction(transaction_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.update_mint_url(old_mint_url, new_mint_url).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn increment_keyset_counter(
        &self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        let res = tx.increment_keyset_counter(keyset_id, count).await?;
        tx.commit().await?;
        Ok(res)
    }

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_mint(mint_url, mint_info).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.remove_mint(mint_url).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_mint_keysets(mint_url, keysets).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_mint_quote(quote).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.remove_mint_quote(quote_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_melt_quote(quote).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.remove_melt_quote(quote_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.add_keys(keyset).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_keys(&self, id: &Id) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        tx.remove_keys(id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        (*tx)
            .kv_write(primary_namespace, secondary_namespace, key, value)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), DatabaseError> {
        let mut tx = self.begin_db_transaction().await?;
        (*tx)
            .kv_remove(primary_namespace, secondary_namespace, key)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct SupabaseWalletTransaction {
    database: SupabaseWalletDatabase,
}

impl SupabaseWalletTransaction {
    /// Get the authorization token to use for requests
    async fn get_auth_bearer(&self) -> String {
        self.database.get_auth_bearer().await
    }

    async fn add_mint(
        &mut self,
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

        let url = self.database.join_url("rest/v1/mint")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&info_table)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add mint: HTTP {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    async fn remove_mint(&mut self, mint_url: MintUrl) -> Result<(), DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/mint?mint_url=eq.{}", mint_url))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove mint: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn update_mint_url(
        &mut self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/mint_quote?mint_url=eq.{}", old_mint_url))?;
        let res = self
            .database
            .client
            .patch(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .json(&serde_json::json!({ "mint_url": new_mint_url.to_string() }))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to update mint url in mint_quote: HTTP {}: {}",
                status, body
            )));
        }

        let url = self
            .database
            .join_url(&format!("rest/v1/proof?mint_url=eq.{}", old_mint_url))?;
        let res = self
            .database
            .client
            .patch(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .json(&serde_json::json!({ "mint_url": new_mint_url.to_string() }))
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to update mint url in proof: HTTP {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    async fn get_keyset_by_id(
        &mut self,
        keyset_id: &Id,
    ) -> Result<Option<KeySetInfo>, DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/keyset?id=eq.{}", keyset_id))?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let items: Vec<KeySetTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            Ok(Some(item.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn get_keys(&mut self, id: &Id) -> Result<Option<Keys>, DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/key?id=eq.{}", id))?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if res.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() || text.trim() == "[]" {
            return Ok(None);
        }
        let items: Vec<KeyTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            Ok(Some(item.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn add_mint_keysets(
        &mut self,
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

        let url = self.database.join_url("rest/v1/keyset?on_conflict=id")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&items)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add mint keysets: HTTP {}: {}",
                status, body
            )));
        }

        Ok(())
    }

    async fn get_mint_quote(&mut self, quote_id: &str) -> Result<Option<MintQuote>, DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/mint_quote?id=eq.{}", quote_id))?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        let items: Vec<MintQuoteTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            Ok(Some(item.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn add_mint_quote(&mut self, quote: MintQuote) -> Result<(), DatabaseError> {
        let item: MintQuoteTable = quote.try_into()?;
        let url = self.database.join_url("rest/v1/mint_quote")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add mint quote: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn remove_mint_quote(&mut self, quote_id: &str) -> Result<(), DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/mint_quote?id=eq.{}", quote_id))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove mint quote: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn get_melt_quote(
        &mut self,
        quote_id: &str,
    ) -> Result<Option<wallet::MeltQuote>, DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/melt_quote?id=eq.{}", quote_id))?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        let items: Vec<MeltQuoteTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            Ok(Some(item.try_into()?))
        } else {
            Ok(None)
        }
    }

    async fn add_melt_quote(&mut self, quote: wallet::MeltQuote) -> Result<(), DatabaseError> {
        let item: MeltQuoteTable = quote.try_into()?;
        let url = self.database.join_url("rest/v1/melt_quote")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add melt quote: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn remove_melt_quote(&mut self, quote_id: &str) -> Result<(), DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/melt_quote?id=eq.{}", quote_id))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove melt quote: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn add_keys(&mut self, keyset: KeySet) -> Result<(), DatabaseError> {
        keyset.verify_id().map_err(DatabaseError::from)?;
        let item = KeyTable::from_keyset(&keyset)?;

        let url = self.database.join_url("rest/v1/key")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add keys: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn remove_keys(&mut self, id: &Id) -> Result<(), DatabaseError> {
        let url = self
            .database
            .join_url(&format!("rest/v1/key?id=eq.{}", id))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove keys: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn get_proofs(
        &mut self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, DatabaseError> {
        let mut query = String::from("rest/v1/proof?select=*");
        if let Some(url) = mint_url {
            query.push_str(&format!("&mint_url=eq.{}", url));
        }
        if let Some(u) = unit {
            query.push_str(&format!("&unit=eq.{}", u));
        }

        if let Some(states) = state {
            let s_str: Vec<String> = states.iter().map(|s| s.to_string()).collect();
            query.push_str(&format!("&state=in.({})", s_str.join(",")));
        }

        let url = self.database.join_url(&query)?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let proofs: Vec<ProofTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        let mut result = Vec::new();
        for p in proofs {
            let info: ProofInfo = p.try_into()?;
            if let Some(_conds) = &spending_conditions {
                // memory filter
                result.push(info);
            } else {
                result.push(info);
            }
        }

        if let Some(conds) = spending_conditions {
            result.retain(|p| {
                if let Some(sc) = &p.spending_condition {
                    conds.contains(sc)
                } else {
                    false
                }
            });
        }

        Ok(result)
    }

    async fn update_proofs(
        &mut self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), DatabaseError> {
        if !added.is_empty() {
            let items: Result<Vec<ProofTable>, DatabaseError> =
                added.into_iter().map(|p| p.try_into()).collect();
            let items = items?;

            let url = self.database.join_url("rest/v1/proof?on_conflict=y")?;

            let res = self
                .database
                .client
                .post(url)
                .header("apikey", &self.database.api_key)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.get_auth_bearer().await),
                )
                .header("Prefer", "resolution=merge-duplicates")
                .json(&items)
                .send()
                .await
                .map_err(Error::Reqwest)?;

            let status = res.status();
            if !status.is_success() {
                let body = res.text().await.map_err(Error::Reqwest)?;
                return Err(DatabaseError::Internal(format!(
                    "HTTP {}: {}",
                    status, body
                )));
            }
        }

        if !removed_ys.is_empty() {
            let ys_str: Vec<String> = removed_ys
                .iter()
                .map(|y| hex::encode(y.to_bytes()))
                .collect();
            let filter = format!("({},)", ys_str.join(","));
            let url = self
                .database
                .join_url(&format!("rest/v1/proof?y=in.{}", filter))?;
            self.database
                .client
                .delete(url)
                .header("apikey", &self.database.api_key)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.get_auth_bearer().await),
                )
                .send()
                .await
                .map_err(Error::Reqwest)?
                .error_for_status()
                .map_err(Error::Reqwest)?;
        }

        Ok(())
    }

    async fn update_proofs_state(
        &mut self,
        ys: Vec<PublicKey>,
        state: State,
    ) -> Result<(), DatabaseError> {
        if ys.is_empty() {
            return Ok(());
        }

        let ys_str: Vec<String> = ys.iter().map(|y| hex::encode(y.to_bytes())).collect();
        let filter = format!("({},)", ys_str.join(","));

        let url = self
            .database
            .join_url(&format!("rest/v1/proof?y=in.{}", filter))?;
        self.database
            .client
            .patch(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .json(&serde_json::json!({ "state": state.to_string() }))
            .send()
            .await
            .map_err(Error::Reqwest)?
            .error_for_status()
            .map_err(Error::Reqwest)?;
        Ok(())
    }

    async fn increment_keyset_counter(
        &mut self,
        keyset_id: &Id,
        count: u32,
    ) -> Result<u32, DatabaseError> {
        // Get current counter value (RLS ensures we only see our own data)
        let current = self.get_keyset_counter(keyset_id).await.unwrap_or(0);
        let new = current + count;

        // For upsert to work with composite primary key (keyset_id, user_id),
        // we need to either:
        // 1. Use DELETE + INSERT (safe with RLS)
        // 2. Use RPC function
        // We'll use DELETE + INSERT approach since RLS ensures we only affect our own rows

        // First, try to delete existing counter (if any) - RLS ensures we only delete our own
        let delete_url = self.database.join_url(&format!(
            "rest/v1/keyset_counter?keyset_id=eq.{}",
            keyset_id
        ))?;
        let _ = self
            .database
            .client
            .delete(delete_url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        // Then insert new counter - user_id will be set by database DEFAULT from JWT
        let item = KeysetCounterTable {
            keyset_id: keyset_id.to_string(),
            counter: new,
            user_id: None, // Will be set by database DEFAULT from JWT's get_current_user_id()
            opt_version: None,
            _extra: Default::default(),
        };

        let insert_url = self.database.join_url("rest/v1/keyset_counter")?;
        let res = self
            .database
            .client
            .post(insert_url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to increment keyset counter: HTTP {}: {}",
                status, body
            )));
        }

        Ok(new)
    }

    async fn add_transaction(&mut self, transaction: Transaction) -> Result<(), DatabaseError> {
        let item: TransactionTable = transaction.try_into()?;
        let url = self.database.join_url("rest/v1/transactions")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to add transaction: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn remove_transaction(
        &mut self,
        transaction_id: TransactionId,
    ) -> Result<(), DatabaseError> {
        let id_hex = transaction_id.to_string();
        let url = self
            .database
            .join_url(&format!("rest/v1/transactions?id=eq.\\x{}", id_hex))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove transaction: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }
}

impl SupabaseWalletTransaction {
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<u32, DatabaseError> {
        let url = self.database.join_url(&format!(
            "rest/v1/keyset_counter?keyset_id=eq.{}",
            keyset_id
        ))?;
        let res = self
            .database
            .client
            .get(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        if !res.status().is_success() {
            return Err(Error::Reqwest(
                res.error_for_status()
                    .expect_err("Status was already checked"),
            )
            .into());
        }
        let text = res.text().await.map_err(Error::Reqwest)?;
        if text.trim().is_empty() {
            return Ok(0);
        }
        let items: Vec<KeysetCounterTable> = serde_json::from_str(&text).map_err(Error::Serde)?;
        if let Some(item) = items.into_iter().next() {
            Ok(item.counter)
        } else {
            Ok(0)
        }
    }
}

#[async_trait]
impl cdk_common::database::KVStoreTransaction<DatabaseError> for SupabaseWalletTransaction {
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DatabaseError> {
        self.database
            .kv_read(primary_namespace, secondary_namespace, key)
            .await
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        self.database
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }

    async fn kv_write(
        &mut self,
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
            user_id: None,
            opt_version: None,
            _extra: Default::default(),
        };

        let url = self.database.join_url("rest/v1/kv_store")?;
        let res = self
            .database
            .client
            .post(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .header("Prefer", "resolution=merge-duplicates")
            .json(&item)
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to write to kv_store: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), DatabaseError> {
        let url = self.database.join_url(&format!(
            "rest/v1/kv_store?primary_namespace=eq.{}&secondary_namespace=eq.{}&key=eq.{}",
            primary_namespace, secondary_namespace, key
        ))?;
        let res = self
            .database
            .client
            .delete(url)
            .header("apikey", &self.database.api_key)
            .header(
                "Authorization",
                format!("Bearer {}", self.get_auth_bearer().await),
            )
            .send()
            .await
            .map_err(Error::Reqwest)?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.map_err(Error::Reqwest)?;
            return Err(DatabaseError::Internal(format!(
                "Failed to remove kv_store entry: HTTP {}: {}",
                status, body
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl DbTransactionFinalizer for SupabaseWalletTransaction {
    type Err = DatabaseError;

    async fn commit(self: Box<Self>) -> Result<(), Self::Err> {
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<(), Self::Err> {
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
    _extra: serde_json::Map<String, serde_json::Value>,
}

impl KeyTable {
    fn from_keyset(keyset: &KeySet) -> Result<Self, DatabaseError> {
        Ok(Self {
            id: keyset.id.to_string(),
            keys: serde_json::to_string(&keyset.keys)?,
            keyset_u32: Some(u32::from(keyset.id) as i64),
            user_id: None,
            opt_version: None,
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
            _extra: Default::default(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KeysetCounterTable {
    keyset_id: String,
    counter: u32,
    // user_id is optional for serialization - if not set, Supabase uses DEFAULT get_current_user_id()
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
    // Extra fields from other applications (ignored during deserialization)
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    user_id: Option<serde_json::Value>,
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    opt_version: Option<serde_json::Value>,
    #[serde(flatten)]
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
            user_id: None,
            opt_version: None,
            _extra: Default::default(),
        })
    }
}
