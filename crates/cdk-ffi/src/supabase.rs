use std::sync::Arc;

use cdk_common::auth::oidc::OidcClient;
use cdk_common::database::Error as CdkDatabaseError;
use cdk_supabase::SupabaseWalletDatabase;

use crate::{
    CurrencyUnit, FfiError, FfiWalletDatabaseWrapper, Id, KeySet, KeySetInfo, Keys, MeltQuote,
    MintInfo, MintQuote, MintUrl, ProofInfo, ProofState, PublicKey, SpendingConditions,
    Transaction, TransactionDirection, TransactionId, WalletDatabase,
};

/// FFI wrapper for Supabase wallet database
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
#[derive(uniffi::Object)]
pub struct WalletSupabaseDatabase {
    inner: Arc<FfiWalletDatabaseWrapper<SupabaseWalletDatabase, CdkDatabaseError>>,
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletSupabaseDatabase {
    /// Create a new WalletSupabaseDatabase with API key only (legacy behavior)
    #[uniffi::constructor]
    pub async fn new(url: String, api_key: String) -> Result<Arc<Self>, FfiError> {
        let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;
        let db = SupabaseWalletDatabase::new(url, api_key)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(Arc::new(WalletSupabaseDatabase {
            inner: FfiWalletDatabaseWrapper::new(db),
        }))
    }

    /// Create a new WalletSupabaseDatabase with OIDC client for automatic token refresh
    #[uniffi::constructor]
    pub async fn with_oidc(
        url: String,
        api_key: String,
        openid_discovery: String,
        client_id: Option<String>,
    ) -> Result<Arc<Self>, FfiError> {
        let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;
        let oidc_client = OidcClient::new(openid_discovery, client_id);
        let db = SupabaseWalletDatabase::with_oidc(url, api_key, oidc_client)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(Arc::new(WalletSupabaseDatabase {
            inner: FfiWalletDatabaseWrapper::new(db),
        }))
    }

    /// Set or update the JWT token for authentication
    pub async fn set_jwt_token(&self, token: Option<String>) {
        self.inner.inner().set_jwt_token(token).await;
    }

    /// Get the current JWT token if set
    pub async fn get_jwt_token(&self) -> Option<String> {
        self.inner.inner().get_jwt_token().await
    }

    /// Set the refresh token for automatic token refresh
    pub async fn set_refresh_token(&self, token: Option<String>) {
        self.inner.inner().set_refresh_token(token).await;
    }

    /// Refresh the access token using the stored refresh token
    ///
    /// This requires both an OIDC client and a refresh token to be set.
    /// On success, the JWT token and expiration are automatically updated.
    ///
    /// Returns an error if:
    /// - No OIDC client is configured
    /// - No refresh token is set
    /// - The refresh token request fails
    pub async fn refresh_access_token(&self) -> Result<(), FfiError> {
        self.inner
            .inner()
            .refresh_access_token()
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    /// Call a Supabase RPC function
    ///
    /// This allows calling any custom PostgreSQL function exposed via Supabase's PostgREST API.
    ///
    /// # Arguments
    /// * `function_name` - The name of the RPC function to call (e.g., "my_function")
    /// * `params_json` - JSON string containing the function parameters (e.g., `{"arg1": "value"}`)
    ///
    /// # Returns
    /// The raw JSON response from Supabase as a string
    ///
    /// # Errors
    /// Returns an error if:
    /// - The params_json is not valid JSON
    /// - The HTTP request fails
    /// - The RPC function returns an error status
    /// ```
    pub async fn call_rpc(
        &self,
        function_name: String,
        params_json: String,
    ) -> Result<String, FfiError> {
        self.inner
            .inner()
            .call_rpc(&function_name, &params_json)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletSupabaseDatabase);
