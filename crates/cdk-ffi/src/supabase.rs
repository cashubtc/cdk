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
    ///
    /// No automatic token refresh is configured. Use `set_jwt_token` to manually
    /// set tokens, or use one of the other constructors for automatic refresh.
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

    /// Create a new WalletSupabaseDatabase with Supabase Auth for automatic token refresh
    ///
    /// This uses Supabase's built-in GoTrue authentication system.
    /// After creation, call `set_jwt_token()` and `set_refresh_token()` with tokens
    /// obtained from Supabase Auth sign-in. Token refresh will use Supabase's
    /// `/auth/v1/token` endpoint automatically.
    #[uniffi::constructor]
    pub async fn with_supabase_auth(url: String, api_key: String) -> Result<Arc<Self>, FfiError> {
        let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;
        let db = SupabaseWalletDatabase::with_supabase_auth(url, api_key)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(Arc::new(WalletSupabaseDatabase {
            inner: FfiWalletDatabaseWrapper::new(db),
        }))
    }

    /// Create a new WalletSupabaseDatabase with external OIDC provider for automatic token refresh
    ///
    /// This uses an external OIDC provider (e.g., Keycloak, Auth0) for token refresh.
    /// The OIDC provider must be configured in Supabase's JWT settings to validate the tokens.
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

    /// Set encryption password
    ///
    /// Derives an encryption key from the password using PBKDF2.
    /// This key is used to encrypt sensitive data (proof secrets, kv_store values)
    /// before sending to Supabase, ensuring end-to-end privacy.
    pub async fn set_encryption_password(&self, password: String) {
        self.inner.inner().set_encryption_password(&password).await;
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

    /// Sign up a new user and automatically set tokens if returned
    pub async fn signup(&self, email: String, password: String) -> Result<AuthResponse, FfiError> {
        let response = self
            .inner
            .inner()
            .signup(&email, &password)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(response.into())
    }

    /// Sign in a user and automatically set tokens on the database instance
    pub async fn signin(&self, email: String, password: String) -> Result<AuthResponse, FfiError> {
        let response = self
            .inner
            .inner()
            .signin(&email, &password)
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(response.into())
    }
}

// Use macro to implement WalletDatabase trait - delegates all methods to inner
crate::impl_ffi_wallet_database!(WalletSupabaseDatabase);

/// Response from Supabase Auth sign-up/sign-in
#[derive(Debug, uniffi::Record)]
pub struct AuthResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<i64>,
    pub refresh_token: Option<String>,
    /// User details as a JSON string
    pub user_json: String,
}

impl From<cdk_supabase::SupabaseAuthResponse> for AuthResponse {
    fn from(r: cdk_supabase::SupabaseAuthResponse) -> Self {
        Self {
            access_token: r.access_token,
            token_type: r.token_type,
            expires_in: r.expires_in,
            refresh_token: r.refresh_token,
            user_json: r.user.to_string(),
        }
    }
}

/// Sign up a new user with email and password
#[uniffi::export(async_runtime = "tokio")]
pub async fn supabase_signup(
    url: String,
    api_key: String,
    email: String,
    password: String,
) -> Result<AuthResponse, FfiError> {
    let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
        error_message: e.to_string(),
    })?;

    let response = cdk_supabase::SupabaseAuth::signup(&url, &api_key, &email, &password)
        .await
        .map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;

    Ok(response.into())
}

/// Sign in a user with email and password
#[uniffi::export(async_runtime = "tokio")]
pub async fn supabase_signin(
    url: String,
    api_key: String,
    email: String,
    password: String,
) -> Result<AuthResponse, FfiError> {
    let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
        error_message: e.to_string(),
    })?;

    let response = cdk_supabase::SupabaseAuth::signin(&url, &api_key, &email, &password)
        .await
        .map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;

    Ok(response.into())
}

/// Run database migrations using the Service Role Key
///
/// This must be called with the Service Role Key to have permission to create tables
/// and RPC functions. Do not use the anon key or an authenticated user token.
#[uniffi::export(async_runtime = "tokio")]
pub async fn supabase_run_migrations(
    url: String,
    service_role_key: String,
) -> Result<(), FfiError> {
    let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
        error_message: e.to_string(),
    })?;

    SupabaseWalletDatabase::run_migrations(url, service_role_key)
        .await
        .map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;

    Ok(())
}

/// Get the full database schema SQL
///
/// Returns the concatenated SQL of all migration files.
/// This can be used to manually set up the database in the Supabase Dashboard.
#[uniffi::export]
pub fn supabase_get_schema_sql() -> String {
    SupabaseWalletDatabase::get_schema_sql()
}
