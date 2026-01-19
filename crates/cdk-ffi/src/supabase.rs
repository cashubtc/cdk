use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::auth::oidc::OidcClient;
use cdk_common::database::{
    wallet::Database, DbTransactionFinalizer, Error as CdkDbError, KVStoreDatabase,
    KVStoreTransaction,
};
use cdk_supabase::SupabaseWalletDatabase;

use crate::{
    CurrencyUnit, FfiError, Id, KeySet, KeySetInfo, Keys, MeltQuote, MintInfo, MintQuote, MintUrl,
    ProofInfo, ProofState, PublicKey, SpendingConditions, Transaction, TransactionDirection,
    TransactionId, WalletDatabase,
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
///
/// ## Automatic Token Synchronization
///
/// For automatic synchronization of CAT tokens with the database, use `wallet.set_supabase_database()`:
///
/// ```ignore
/// // Create database without JWT token initially
/// let db = WalletSupabaseDatabase::new(url, api_key)?;
///
/// // Create wallet with the database
/// let wallet = Wallet::new(mint_url, unit, mnemonic, db.clone(), config)?;
///
/// // Register the database for automatic token sync
/// wallet.set_supabase_database(db).await;
///
/// // Now when you set CAT token, it automatically syncs to Supabase
/// wallet.set_cat(cat_token).await?;
/// wallet.set_refresh_token(refresh_token).await?;
///
/// // Token refresh also syncs automatically
/// wallet.refresh_access_token().await?;
/// ```
///
/// ## Manual Token Management
///
/// You can also manually manage tokens using `set_jwt_token()` if needed:
///
/// ```ignore
/// db.set_jwt_token(Some(jwt_token)).await;
/// ```
#[derive(uniffi::Object)]
pub struct WalletSupabaseDatabase {
    inner: SupabaseWalletDatabase,
}

#[uniffi::export]
impl WalletSupabaseDatabase {
    /// Create a new WalletSupabaseDatabase with API key only (legacy behavior)
    #[uniffi::constructor]
    pub fn new(url: String, api_key: String) -> Result<Arc<Self>, FfiError> {
        let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;
        let inner = SupabaseWalletDatabase::new(url, api_key);
        Ok(Arc::new(WalletSupabaseDatabase { inner }))
    }

    /// Create a new WalletSupabaseDatabase with OIDC client for automatic token refresh
    #[uniffi::constructor]
    pub fn with_oidc(
        url: String,
        api_key: String,
        openid_discovery: String,
        client_id: Option<String>,
    ) -> Result<Arc<Self>, FfiError> {
        let url = url::Url::parse(&url).map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })?;
        let oidc_client = OidcClient::new(openid_discovery, client_id);
        let inner = SupabaseWalletDatabase::with_oidc(url, api_key, oidc_client);
        Ok(Arc::new(WalletSupabaseDatabase { inner }))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletSupabaseDatabase {
    /// Set or update the JWT token for authentication
    pub async fn set_jwt_token(&self, token: Option<String>) {
        self.inner.set_jwt_token(token).await;
    }

    /// Get the current JWT token if set
    pub async fn get_jwt_token(&self) -> Option<String> {
        self.inner.get_jwt_token().await
    }

    /// Set the refresh token for automatic token refresh
    pub async fn set_refresh_token(&self, token: Option<String>) {
        self.inner.set_refresh_token(token).await;
    }

    /// Set the token expiration time (Unix timestamp in seconds)
    pub async fn set_token_expiration(&self, expiration: Option<u64>) {
        self.inner.set_token_expiration(expiration).await;
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
            .refresh_access_token()
            .await
            .map_err(|e| FfiError::Internal {
                error_message: e.to_string(),
            })
    }
}

#[uniffi::export(async_runtime = "tokio")]
#[async_trait::async_trait]
impl WalletDatabase for WalletSupabaseDatabase {
    // ========== Read methods ==========

    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, FfiError> {
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            ys.into_iter().map(|y| y.try_into()).collect();
        let cdk_ys = cdk_ys?;

        let result = Database::get_proofs_by_ys(&self.inner, cdk_ys)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let result = Database::get_mint(&self.inner, cdk_mint_url)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(Into::into))
    }

    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, FfiError> {
        let result = Database::get_mints(&self.inner)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result
            .into_iter()
            .map(|(k, v)| (k.into(), v.map(Into::into)))
            .collect())
    }

    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let result = Database::get_mint_keysets(&self.inner, cdk_mint_url)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(|keysets| keysets.into_iter().map(Into::into).collect()))
    }

    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, FfiError> {
        let cdk_id = keyset_id.into();
        let result = Database::get_keyset_by_id(&self.inner, &cdk_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(Into::into))
    }

    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, FfiError> {
        let result = Database::get_mint_quote(&self.inner, &quote_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(|q| q.into()))
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        let result = Database::get_mint_quotes(&self.inner)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        let result = Database::get_unissued_mint_quotes(&self.inner)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, FfiError> {
        let result = Database::get_melt_quote(&self.inner, &quote_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(|q| q.into()))
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, FfiError> {
        let result = Database::get_melt_quotes(&self.inner)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.into_iter().map(|q| q.into()).collect())
    }

    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, FfiError> {
        let cdk_id = id.into();
        let result = Database::get_keys(&self.inner, &cdk_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(Into::into))
    }

    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_unit = unit.map(Into::into);
        let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let cdk_spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>> =
            spending_conditions
                .map(|sc| {
                    sc.into_iter()
                        .map(|c| c.try_into())
                        .collect::<Result<Vec<_>, FfiError>>()
                })
                .transpose()?;

        let result = Database::get_proofs(
            &self.inner,
            cdk_mint_url,
            cdk_unit,
            cdk_state,
            cdk_spending_conditions,
        )
        .await
        .map_err(|e: CdkDbError| FfiError::Internal {
            error_message: e.to_string(),
        })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_unit = unit.map(Into::into);
        let cdk_state = state.map(|s| s.into_iter().map(Into::into).collect());

        Database::get_balance(&self.inner, cdk_mint_url, cdk_unit, cdk_state)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, FfiError> {
        let cdk_id = transaction_id.try_into()?;
        let result = Database::get_transaction(&self.inner, cdk_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(result.map(Into::into))
    }

    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, FfiError> {
        let cdk_mint_url = mint_url.map(|u| u.try_into()).transpose()?;
        let cdk_direction = direction.map(Into::into);
        let cdk_unit = unit.map(Into::into);

        let result =
            Database::list_transactions(&self.inner, cdk_mint_url, cdk_direction, cdk_unit)
                .await
                .map_err(|e: CdkDbError| FfiError::Internal {
                    error_message: e.to_string(),
                })?;

        Ok(result.into_iter().map(Into::into).collect())
    }

    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, FfiError> {
        let result: Result<Option<Vec<u8>>, CdkDbError> =
            <SupabaseWalletDatabase as KVStoreDatabase>::kv_read(
                &self.inner,
                &primary_namespace,
                &secondary_namespace,
                &key,
            )
            .await;
        result.map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })
    }

    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, FfiError> {
        let result: Result<Vec<String>, CdkDbError> =
            <SupabaseWalletDatabase as KVStoreDatabase>::kv_list(
                &self.inner,
                &primary_namespace,
                &secondary_namespace,
            )
            .await;
        result.map_err(|e| FfiError::Internal {
            error_message: e.to_string(),
        })
    }

    async fn kv_write(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), FfiError> {
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        (*tx)
            .kv_write(&primary_namespace, &secondary_namespace, &key, &value)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        tx.commit()
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(())
    }

    async fn kv_remove(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<(), FfiError> {
        let mut tx = self
            .inner
            .begin_db_transaction()
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        (*tx)
            .kv_remove(&primary_namespace, &secondary_namespace, &key)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        tx.commit()
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })?;
        Ok(())
    }

    // ========== Write methods ==========

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), FfiError> {
        let cdk_added: Result<Vec<cdk::types::ProofInfo>, FfiError> = added
            .into_iter()
            .map(|info| {
                Ok::<cdk::types::ProofInfo, FfiError>(cdk::types::ProofInfo {
                    proof: info.proof.try_into()?,
                    y: info.y.try_into()?,
                    mint_url: info.mint_url.try_into()?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()?,
                    unit: info.unit.into(),
                })
            })
            .collect();
        let cdk_added = cdk_added?;

        let cdk_removed_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            removed_ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_removed_ys = cdk_removed_ys?;

        Database::update_proofs(&self.inner, cdk_added, cdk_removed_ys)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), FfiError> {
        let cdk_ys: Result<Vec<cdk::nuts::PublicKey>, FfiError> =
            ys.into_iter().map(|pk| pk.try_into()).collect();
        let cdk_ys = cdk_ys?;
        let cdk_state = state.into();

        Database::update_proofs_state(&self.inner, cdk_ys, cdk_state)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_transaction(&self, transaction: Transaction) -> Result<(), FfiError> {
        let cdk_transaction: cdk::wallet::types::Transaction = transaction.try_into()?;
        Database::add_transaction(&self.inner, cdk_transaction)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), FfiError> {
        let cdk_id = transaction_id.try_into()?;
        Database::remove_transaction(&self.inner, cdk_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), FfiError> {
        let cdk_old = old_mint_url.try_into()?;
        let cdk_new = new_mint_url.try_into()?;
        Database::update_mint_url(&self.inner, cdk_old, cdk_new)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, FfiError> {
        let cdk_id = keyset_id.into();
        Database::increment_keyset_counter(&self.inner, &cdk_id, count)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let cdk_mint_info = mint_info.map(Into::into);
        Database::add_mint(&self.inner, cdk_mint_url, cdk_mint_info)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        Database::remove_mint(&self.inner, cdk_mint_url)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), FfiError> {
        let cdk_mint_url = mint_url.try_into()?;
        let cdk_keysets: Vec<cdk::nuts::KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        Database::add_mint_keysets(&self.inner, cdk_mint_url, cdk_keysets)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), FfiError> {
        let cdk_quote = quote.try_into()?;
        Database::add_mint_quote(&self.inner, cdk_quote)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), FfiError> {
        Database::remove_mint_quote(&self.inner, &quote_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), FfiError> {
        let cdk_quote = quote.try_into()?;
        Database::add_melt_quote(&self.inner, cdk_quote)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), FfiError> {
        Database::remove_melt_quote(&self.inner, &quote_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn add_keys(&self, keyset: KeySet) -> Result<(), FfiError> {
        let cdk_keyset: cdk::nuts::KeySet = keyset.try_into()?;
        Database::add_keys(&self.inner, cdk_keyset)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }

    async fn remove_keys(&self, id: Id) -> Result<(), FfiError> {
        let cdk_id = id.into();
        Database::remove_keys(&self.inner, &cdk_id)
            .await
            .map_err(|e: CdkDbError| FfiError::Internal {
                error_message: e.to_string(),
            })
    }
}
