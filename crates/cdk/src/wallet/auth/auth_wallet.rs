use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::wallet::ProofInfo;
use cdk_common::{AuthProof, Id, Keys, MintInfo};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::instrument;

use super::AuthMintConnector;
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::{
    nut12, AuthRequired, AuthToken, BlindAuthToken, CurrencyUnit, KeySetInfo, PreMintSecrets,
    Proofs, ProtectedEndpoint, State,
};
use crate::wallet::mint_connector::AuthHttpClient;
use crate::wallet::mint_metadata_cache::MintMetadataCache;
use crate::{Amount, Error, OidcClient};

/// JWT Claims structure for decoding tokens
#[derive(Debug, Serialize, Deserialize)]
struct _Claims {
    /// Subject
    sub: Option<String>,
    /// Expiration time (as UTC timestamp)
    exp: Option<u64>,
    /// Issued at (as UTC timestamp)
    iat: Option<u64>,
}
/// CDK Auth Wallet
///
/// A [`AuthWallet`] is for auth operations with a single mint.
#[derive(Debug, Clone)]
pub struct AuthWallet {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    /// Mint metadata cache (lock-free cached access to keys, keysets, and mint info)
    pub metadata_cache: Arc<MintMetadataCache>,
    /// Protected methods
    pub protected_endpoints: Arc<RwLock<HashMap<ProtectedEndpoint, AuthRequired>>>,
    /// Refresh token for auth
    refresh_token: Arc<RwLock<Option<String>>>,
    auth_client: Arc<dyn AuthMintConnector + Send + Sync>,
    /// OIDC client for authentication
    oidc_client: Arc<RwLock<Option<OidcClient>>>,
}

impl AuthWallet {
    /// Create a new [`AuthWallet`] instance
    pub fn new(
        mint_url: MintUrl,
        cat: Option<AuthToken>,
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        metadata_cache: Arc<MintMetadataCache>,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
        oidc_client: Option<OidcClient>,
    ) -> Self {
        let http_client = Arc::new(AuthHttpClient::new(mint_url.clone(), cat));
        Self {
            mint_url,
            localstore,
            metadata_cache,
            protected_endpoints: Arc::new(RwLock::new(protected_endpoints)),
            refresh_token: Arc::new(RwLock::new(None)),
            auth_client: http_client,
            oidc_client: Arc::new(RwLock::new(oidc_client)),
        }
    }

    /// Get the current auth token
    #[instrument(skip(self))]
    pub async fn get_auth_token(&self) -> Result<AuthToken, Error> {
        self.auth_client.get_auth_token().await
    }

    /// Set a new auth token
    #[instrument(skip_all)]
    pub async fn verify_cat(&self, token: AuthToken) -> Result<(), Error> {
        match &token {
            AuthToken::ClearAuth(clear_token) => {
                if let Some(oidc) = self.oidc_client.read().await.as_ref() {
                    oidc.verify_cat(clear_token).await?;
                }
                Ok(())
            }
            AuthToken::BlindAuth(_) => Err(Error::Custom(
                "Cannot set blind auth token directly".to_string(),
            )),
        }
    }

    /// Set a new auth token
    #[instrument(skip_all)]
    pub async fn set_auth_token(&self, token: AuthToken) -> Result<(), Error> {
        match &token {
            AuthToken::ClearAuth(clear_token) => {
                if let Some(oidc) = self.oidc_client.read().await.as_ref() {
                    oidc.verify_cat(clear_token).await?;
                }
                self.auth_client.set_auth_token(token).await
            }
            AuthToken::BlindAuth(_) => Err(Error::Custom(
                "Cannot set blind auth token directly".to_string(),
            )),
        }
    }

    /// Get the current refresh token if one exists
    #[instrument(skip(self))]
    pub async fn get_refresh_token(&self) -> Option<String> {
        self.refresh_token.read().await.clone()
    }

    /// Set a new refresh token
    #[instrument(skip(self))]
    pub async fn set_refresh_token(&self, token: Option<String>) {
        *self.refresh_token.write().await = token;
    }

    /// Get the OIDC client if one exists
    #[instrument(skip(self))]
    pub async fn get_oidc_client(&self) -> Option<OidcClient> {
        self.oidc_client.read().await.clone()
    }

    /// Set a new OIDC client
    #[instrument(skip(self))]
    pub async fn set_oidc_client(&self, client: Option<OidcClient>) {
        *self.oidc_client.write().await = client;
    }

    /// Refresh the access token using the stored refresh token
    #[instrument(skip(self))]
    pub async fn refresh_access_token(&self) -> Result<(), Error> {
        if let Some(oidc) = self.oidc_client.read().await.as_ref() {
            if let Some(refresh_token) = self.get_refresh_token().await {
                let mint_info = self
                    .get_mint_info()
                    .await?
                    .ok_or(Error::CouldNotGetMintInfo)?;
                let token_response = oidc
                    .refresh_access_token(
                        mint_info.client_id().ok_or(Error::CouldNotGetMintInfo)?,
                        refresh_token,
                    )
                    .await?;

                // Store new refresh token if provided
                self.set_refresh_token(token_response.refresh_token).await;

                // Set new access token
                self.set_auth_token(AuthToken::ClearAuth(token_response.access_token))
                    .await?;

                return Ok(());
            }
        }
        Err(Error::Custom(
            "No refresh token or OIDC client available".to_string(),
        ))
    }

    /// Query mint for current mint information
    #[instrument(skip(self))]
    pub async fn get_mint_info(&self) -> Result<Option<MintInfo>, Error> {
        self.auth_client
            .get_mint_info()
            .await
            .map(Some)
            .or(Ok(None))
    }

    /// Fetch keys for mint keyset
    ///
    /// Returns keys from metadata cache if available, fetches from mint if not.
    #[instrument(skip(self))]
    pub async fn load_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let metadata = self
            .metadata_cache
            .load_auth(&self.localstore, &self.auth_client)
            .await?;

        match metadata.keysets.get(&keyset_id) {
            Some(keyset) if keyset.unit == CurrencyUnit::Auth => (),
            _ => return Err(Error::UnknownKeySet),
        }

        metadata
            .keys
            .get(&keyset_id)
            .map(|x| (*(x.clone())).clone())
            .ok_or(Error::UnknownKeySet)
    }

    /// Get blind auth keysets from metadata cache
    ///
    /// Checks the metadata cache for auth keysets. If cache is not populated,
    /// fetches from the mint server and updates the cache.
    /// This is the main method for getting auth keysets in operations that can work offline
    /// but will fall back to online if needed.
    #[instrument(skip(self))]
    pub async fn load_mint_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let metadata = self
            .metadata_cache
            .load_auth(&self.localstore, &self.auth_client)
            .await?;

        let auth_keysets = metadata
            .keysets
            .values()
            .filter_map(|k| {
                if k.unit == CurrencyUnit::Auth {
                    Some((*(k.clone())).clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        if !auth_keysets.is_empty() {
            Ok(auth_keysets)
        } else {
            Err(Error::UnknownKeySet)
        }
    }

    /// Refresh blind auth keysets by fetching the latest from mint
    ///
    /// Fetches the latest blind auth keyset information from the mint server,
    /// updating the metadata cache and database. Returns only the keysets with
    /// Auth currency unit. Use this when you need the most up-to-date keyset information.
    #[instrument(skip(self))]
    pub async fn refresh_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        tracing::debug!("Refreshing auth keysets from mint");

        self.load_mint_keysets().await
    }

    /// Get the first active blind auth keyset - always goes online
    ///
    /// This method always goes online to refresh keysets from the mint and then returns
    /// the first active keyset found. Use this when you need the most up-to-date
    /// keyset information for blind auth operations.
    #[instrument(skip(self))]
    pub async fn fetch_active_keyset(&self) -> Result<KeySetInfo, Error> {
        let auth_keysets = self.refresh_keysets().await?;
        let keyset = auth_keysets.first().ok_or(Error::NoActiveKeyset)?;
        Ok(keyset.clone())
    }

    /// Get unspent auth proofs from local database only - offline operation
    ///
    /// Returns auth proofs from the local database that are in the Unspent state.
    /// This is an offline operation that does not contact the mint.
    #[instrument(skip(self))]
    pub async fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(CurrencyUnit::Auth),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof.try_into())
            .collect::<Result<Vec<AuthProof>, _>>()?)
    }

    /// Check if and what kind of auth is required for a method
    #[instrument(skip(self))]
    pub async fn is_protected(&self, method: &ProtectedEndpoint) -> Option<AuthRequired> {
        let protected_endpoints = self.protected_endpoints.read().await;

        protected_endpoints.get(method).copied().or_else(|| {
            protected_endpoints
                .iter()
                .filter_map(|(endpoint, auth)| {
                    endpoint
                        .match_specificity(method)
                        .map(|specificity| (specificity, *auth))
                })
                .max_by_key(|(specificity, _)| *specificity)
                .map(|(_, auth)| auth)
        })
    }

    /// Get Auth Token
    #[instrument(skip(self))]
    pub async fn get_blind_auth_token(&self) -> Result<Option<BlindAuthToken>, Error> {
        let auth_proof = match self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(CurrencyUnit::Auth),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .pop()
        {
            Some(proof) => {
                self.localstore
                    .update_proofs(vec![], vec![proof.proof.y()?])
                    .await?;
                proof.proof.try_into()?
            }
            None => return Ok(None),
        };

        Ok(Some(BlindAuthToken { auth_proof }))
    }

    /// Auth for request
    #[instrument(skip(self))]
    pub async fn get_auth_for_request(
        &self,
        method: &ProtectedEndpoint,
    ) -> Result<Option<AuthToken>, Error> {
        match self.is_protected(method).await {
            Some(auth) => match auth {
                AuthRequired::Clear => {
                    tracing::trace!("Clear auth needed for request.");
                    self.auth_client.get_auth_token().await.map(Some)
                }
                AuthRequired::Blind => {
                    tracing::trace!("Blind auth needed for request getting Auth proof.");
                    let proof = self.get_blind_auth_token().await?.ok_or_else(|| {
                        tracing::debug!(
                            "Insufficient blind auth proofs in wallet. Must mint bats."
                        );
                        Error::InsufficientBlindAuthTokens
                    })?;

                    let auth_token = AuthToken::BlindAuth(proof.without_dleq());

                    Ok(Some(auth_token))
                }
            },
            None => Ok(None),
        }
    }

    /// Mint blind auth
    #[instrument(skip(self))]
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Error> {
        tracing::debug!("Minting {} blind auth proofs", amount);

        let auth_token = self.auth_client.get_auth_token().await?;

        match &auth_token {
            AuthToken::ClearAuth(cat) => {
                if cat.is_empty() {
                    tracing::warn!("Auth Cat is not set");
                    return Err(Error::ClearAuthRequired);
                }

                if let Err(err) = self.verify_cat(auth_token).await {
                    tracing::warn!("Current cat is invalid {}", err);
                }

                let has_refresh;

                {
                    has_refresh = self.refresh_token.read().await.is_some();
                }

                if has_refresh {
                    tracing::info!("Attempting to refresh using refresh token");
                    self.refresh_access_token().await?;
                } else {
                    tracing::warn!(
                        "Wallet cat is invalid and there is no refresh token please reauth"
                    );
                }
            }
            AuthToken::BlindAuth(_) => {
                tracing::error!("Blind auth set as client cat");
                return Err(Error::ClearAuthFailed);
            }
        }

        let keysets = self
            .load_mint_keysets()
            .await?
            .into_iter()
            .map(|x| (x.id, x))
            .collect::<HashMap<_, _>>();

        let active_keyset_id = self.fetch_active_keyset().await?.id;
        let fee_and_amounts = (
            keysets
                .get(&active_keyset_id)
                .map(|x| x.input_fee_ppk)
                .unwrap_or(0),
            self.load_keyset_keys(active_keyset_id)
                .await?
                .iter()
                .map(|(amount, _)| amount.to_u64())
                .collect::<Vec<_>>(),
        )
            .into();

        let premint_secrets = PreMintSecrets::random(
            active_keyset_id,
            amount,
            &SplitTarget::Value(1.into()),
            &fee_and_amounts,
        )?;

        let request = MintAuthRequest {
            outputs: premint_secrets.blinded_messages(),
        };

        let mint_res = self.auth_client.post_mint_blind_auth(request).await?;

        let keys = self.load_keyset_keys(active_keyset_id).await?;

        // Verify the signature DLEQ is valid
        {
            if mint_res.signatures.len() != premint_secrets.secrets.len() {
                return Err(Error::InvalidMintResponse(format!(
                    "mint auth signatures ({}) does not match secrets sent ({})",
                    mint_res.signatures.len(),
                    premint_secrets.secrets.len()
                )));
            }
            for (sig, premint) in mint_res.signatures.iter().zip(&premint_secrets.secrets) {
                if sig.amount != premint.blinded_message.amount {
                    return Err(Error::InvalidMintResponse(format!(
                        "mint auth signature amount ({}) does not match requested amount ({})",
                        sig.amount, premint.blinded_message.amount
                    )));
                }

                if sig.keyset_id != premint.blinded_message.keyset_id {
                    return Err(Error::InvalidMintResponse(format!(
                        "mint auth signature keyset ({}) does not match requested keyset ({})",
                        sig.keyset_id, premint.blinded_message.keyset_id
                    )));
                }

                let keys = self.load_keyset_keys(sig.keyset_id).await?;
                let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
                match sig.verify_dleq(key, premint.blinded_message.blinded_secret) {
                    Ok(_) => (),
                    Err(nut12::Error::MissingDleqProof) => {
                        tracing::warn!("Signature for bat returned without dleq proof.");
                        return Err(Error::DleqProofNotProvided);
                    }
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
        }

        let proofs = construct_proofs(
            mint_res.signatures,
            premint_secrets.rs(),
            premint_secrets.secrets(),
            &keys,
        )?;

        let proof_infos = proofs
            .clone()
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    self.mint_url.clone(),
                    State::Unspent,
                    crate::nuts::CurrencyUnit::Auth,
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        // Add new proofs to store
        self.localstore.update_proofs(proof_infos, vec![]).await?;

        Ok(proofs)
    }

    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_blind_auth_balance(&self) -> Result<Amount, Error> {
        Ok(Amount::from(
            self.get_unspent_auth_proofs().await?.len() as u64
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use async_trait::async_trait;
    use bitcoin::bip32::DerivationPath;
    use bitcoin::secp256k1::Secp256k1;
    use cdk_common::mint_url::MintUrl;
    use cdk_common::nut02::KeySetVersion;
    use cdk_common::nuts::{BlindSignature, KeySet, KeysetResponse, MintKeySet, MintResponse};

    use super::*;
    use crate::nuts::{Method, RoutePath};

    async fn auth_wallet(
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> AuthWallet {
        let mint_url = MintUrl::from_str("https://test-mint.example.com")
            .expect("test mint URL should be valid");
        let localstore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("in-memory wallet database should initialize"),
        );
        let metadata_cache = Arc::new(MintMetadataCache::new(mint_url.clone()));

        AuthWallet::new(
            mint_url,
            None,
            localstore,
            metadata_cache,
            protected_endpoints,
            None,
        )
    }

    fn build_auth_keyset(seed_byte: u8) -> KeySet {
        let secp = Secp256k1::new();
        let seed = [seed_byte; 32];
        let path = DerivationPath::from_str("m/0'").expect("valid derivation path");
        let mint_keyset = MintKeySet::generate_from_seed(
            &secp,
            &seed,
            &[1, 2],
            CurrencyUnit::Auth,
            path,
            0,
            None,
            KeySetVersion::Version00,
        );

        KeySet {
            id: mint_keyset.id,
            unit: mint_keyset.unit.clone(),
            active: None,
            keys: mint_keyset.keys.into(),
            input_fee_ppk: mint_keyset.input_fee_ppk,
            final_expiry: mint_keyset.final_expiry,
        }
    }

    #[derive(Debug)]
    struct MockAuthConnector {
        keyset_active: KeySet,
        keyset_inactive: KeySet,
        respond_with_unrequested_keyset: bool,
    }

    #[async_trait]
    impl AuthMintConnector for MockAuthConnector {
        async fn get_auth_token(&self) -> Result<AuthToken, Error> {
            Ok(AuthToken::ClearAuth("dummy".to_string()))
        }

        async fn set_auth_token(&self, _: AuthToken) -> Result<(), Error> {
            Ok(())
        }

        async fn get_mint_info(&self) -> Result<MintInfo, Error> {
            Ok(MintInfo::default())
        }

        async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
            match keyset_id {
                id if id == self.keyset_active.id => Ok(self.keyset_active.clone()),
                id if id == self.keyset_inactive.id => Ok(self.keyset_inactive.clone()),
                _ => Err(Error::UnknownKeySet),
            }
        }

        async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error> {
            Ok(KeysetResponse {
                keysets: vec![
                    KeySetInfo {
                        id: self.keyset_active.id,
                        unit: CurrencyUnit::Auth,
                        active: true,
                        input_fee_ppk: 0,
                        final_expiry: None,
                    },
                    KeySetInfo {
                        id: self.keyset_inactive.id,
                        unit: CurrencyUnit::Auth,
                        active: false,
                        input_fee_ppk: 0,
                        final_expiry: None,
                    },
                ],
            })
        }

        async fn post_mint_blind_auth(
            &self,
            request: MintAuthRequest,
        ) -> Result<MintResponse, Error> {
            let signatures = request
                .outputs
                .into_iter()
                .map(|output| BlindSignature {
                    amount: output.amount,
                    keyset_id: if self.respond_with_unrequested_keyset {
                        match output.keyset_id {
                            id if id == self.keyset_active.id => self.keyset_inactive.id,
                            _ => self.keyset_active.id,
                        }
                    } else {
                        output.keyset_id
                    },
                    c: output.blinded_secret,
                    dleq: None,
                })
                .collect();

            Ok(MintResponse { signatures })
        }
    }

    async fn auth_wallet_with_connector(
        connector: Arc<dyn AuthMintConnector + Send + Sync>,
    ) -> AuthWallet {
        let mint_url = MintUrl::from_str("https://test-mint.example.com")
            .expect("test mint URL should be valid");
        let localstore = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("in-memory wallet database should initialize"),
        );
        let metadata_cache = Arc::new(MintMetadataCache::new(mint_url.clone()));

        AuthWallet {
            mint_url,
            localstore,
            metadata_cache,
            protected_endpoints: Arc::new(RwLock::new(HashMap::new())),
            refresh_token: Arc::new(RwLock::new(None)),
            auth_client: connector,
            oidc_client: Arc::new(RwLock::new(None)),
        }
    }

    #[tokio::test]
    async fn load_keyset_keys_returns_keys_for_requested_id() {
        let keyset_active = build_auth_keyset(1);
        let keyset_inactive = build_auth_keyset(2);
        assert_ne!(keyset_active.id, keyset_inactive.id);
        assert_ne!(keyset_active.keys, keyset_inactive.keys);

        let wallet = auth_wallet_with_connector(Arc::new(MockAuthConnector {
            keyset_active,
            keyset_inactive: keyset_inactive.clone(),
            respond_with_unrequested_keyset: false,
        }))
        .await;

        let loaded = wallet
            .load_keyset_keys(keyset_inactive.id)
            .await
            .expect("load_keyset_keys should succeed");

        assert_eq!(loaded, keyset_inactive.keys);
    }

    #[tokio::test]
    async fn mint_blind_auth_rejects_signature_for_unrequested_keyset() {
        let keyset_active = build_auth_keyset(1);
        let keyset_inactive = build_auth_keyset(2);

        let wallet = auth_wallet_with_connector(Arc::new(MockAuthConnector {
            keyset_active,
            respond_with_unrequested_keyset: true,
            keyset_inactive,
        }))
        .await;

        let err = wallet
            .mint_blind_auth(Amount::from(1))
            .await
            .expect_err("mismatched keyset response should fail");

        assert!(
            matches!(err, Error::InvalidMintResponse(ref message) if message.contains("keyset")),
            "expected keyset mismatch error, got {err}"
        );
    }

    #[tokio::test]
    async fn is_protected_matches_wildcard_by_specificity() {
        let request =
            ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote("bolt11".to_string()));
        let exact =
            ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote("bolt11".to_string()));
        let broad_wildcard =
            ProtectedEndpoint::new(Method::Post, RoutePath::Wildcard("/v1/".to_string()));
        let specific_wildcard = ProtectedEndpoint::new(
            Method::Post,
            RoutePath::Wildcard("/v1/mint/quote/".to_string()),
        );
        let different_method_wildcard = ProtectedEndpoint::new(
            Method::Get,
            RoutePath::Wildcard("/v1/mint/quote/".to_string()),
        );

        let wallet = auth_wallet(HashMap::from([
            (broad_wildcard, AuthRequired::Clear),
            (specific_wildcard, AuthRequired::Blind),
            (different_method_wildcard, AuthRequired::Clear),
        ]))
        .await;

        assert_eq!(
            wallet.is_protected(&request).await,
            Some(AuthRequired::Blind)
        );

        wallet
            .protected_endpoints
            .write()
            .await
            .insert(exact, AuthRequired::Clear);

        assert_eq!(
            wallet.is_protected(&request).await,
            Some(AuthRequired::Clear)
        );
    }
}
