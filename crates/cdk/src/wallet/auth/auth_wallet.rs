use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::{self, WalletDatabase};
use cdk_common::mint_url::MintUrl;
use cdk_common::util::unix_time;
use cdk_common::{Id, Keys, MintInfo};
use tokio::sync::RwLock;
use tracing::instrument;

use super::AuthMintConnector;
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut22::MintAuthRequest;
use crate::nuts::{
    AuthRequired, AuthToken, BlindAuthToken, CurrencyUnit, KeySetInfo, PreMintSecrets, Proofs,
    ProtectedEndpoint, State,
};
use crate::types::ProofInfo;
use crate::wallet::mint_connector::AuthHttpClient;
use crate::{Amount, Error};
/// CDK Auth Wallet
///
/// A [`AuthWallet`] is for auth operations with a single mint.
#[derive(Debug, Clone)]
pub struct AuthWallet {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
    /// Clear Auth token
    pub cat: Arc<RwLock<Option<String>>>,
    /// Protected methods
    pub protected_endpoints: Arc<RwLock<HashMap<ProtectedEndpoint, AuthRequired>>>,
    client: Arc<dyn AuthMintConnector + Send + Sync>,
}

impl AuthWallet {
    /// Create a new [`AuthWallet`] instance
    pub fn new(
        mint_url: MintUrl,
        // TODO: This should be changed to support adding user password and then that will get the cat
        cat: AuthToken,
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> Self {
        let http_client = Arc::new(AuthHttpClient::new(mint_url.clone(), cat.clone()));
        Self {
            mint_url,
            localstore,
            cat: Arc::new(RwLock::new(Some(cat.to_string()))),
            protected_endpoints: Arc::new(RwLock::new(protected_endpoints)),
            client: http_client,
        }
    }

    /// Query mint for current mint information
    #[instrument(skip(self))]
    pub async fn get_mint_info(&self) -> Result<Option<MintInfo>, Error> {
        match self.client.get_mint_info().await {
            Ok(mint_info) => {
                // If mint provides time make sure it is accurate
                if let Some(mint_unix_time) = mint_info.time {
                    let current_unix_time = unix_time();
                    if current_unix_time.abs_diff(mint_unix_time) > 30 {
                        tracing::warn!(
                            "Mint time does match wallet time. Mint: {}, Wallet: {}",
                            mint_unix_time,
                            current_unix_time
                        );
                        return Err(Error::MintTimeExceedsTolerance);
                    }
                }

                self.localstore
                    .add_mint(self.mint_url.clone(), Some(mint_info.clone()))
                    .await?;

                tracing::trace!("Mint info updated for {}", self.mint_url);

                Ok(Some(mint_info))
            }
            Err(err) => {
                tracing::warn!("Could not get mint info {}", err);
                Ok(None)
            }
        }
    }

    /// Get keys for mint keyset
    ///
    /// Selected keys from localstore if they are already known
    /// If they are not known queries mint for keyset id and stores the [`Keys`]
    #[instrument(skip(self))]
    pub async fn get_keyset_keys(&self, keyset_id: Id) -> Result<Keys, Error> {
        let keys = if let Some(keys) = self.localstore.get_keys(&keyset_id).await? {
            keys
        } else {
            let keys = self.client.get_mint_blind_auth_keyset(keyset_id).await?;

            keys.verify_id()?;

            self.localstore.add_keys(keys.keys.clone()).await?;

            keys.keys
        };

        Ok(keys)
    }

    /// Get active keyset for mint
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_blind_auth_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_blind_auth_keysets().await?;
        let keysets = keysets.keysets;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.clone())
            .await?;

        let active_keysets = keysets
            .clone()
            .into_iter()
            .filter(|k| k.unit == CurrencyUnit::Auth)
            .collect::<Vec<KeySetInfo>>();

        match self
            .localstore
            .get_mint_keysets(self.mint_url.clone())
            .await?
        {
            Some(known_keysets) => {
                let unknown_keysets: Vec<&KeySetInfo> = keysets
                    .iter()
                    .filter(|k| known_keysets.contains(k))
                    .collect();

                for keyset in unknown_keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
            None => {
                for keyset in keysets {
                    self.get_keyset_keys(keyset.id).await?;
                }
            }
        }
        Ok(active_keysets)
    }

    /// Get active keyset for mint
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_blind_auth_keyset(&self) -> Result<KeySetInfo, Error> {
        let active_keysets = self.get_active_mint_blind_auth_keysets().await?;

        let keyset = active_keysets.first().ok_or(Error::NoActiveKeyset)?;
        Ok(keyset.clone())
    }

    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_unspent_auth_proofs(&self) -> Result<Proofs, Error> {
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
            .map(|p| p.proof)
            .collect())
    }

    /// Check if and what kind of auth is required for a method
    pub async fn is_protected(&self, method: &ProtectedEndpoint) -> Option<AuthRequired> {
        let protected_endpoints = self.protected_endpoints.read().await;
        protected_endpoints.get(method).copied()
    }

    /// Get Auth Token
    pub async fn get_blind_auth_token(&self) -> Result<Option<AuthToken>, Error> {
        let unspent = self.get_unspent_auth_proofs().await?;

        let auth_proof = match unspent.first() {
            Some(proof) => {
                self.localstore
                    .update_proofs(vec![], vec![proof.y()?])
                    .await?;
                proof
            }
            None => return Ok(None),
        };

        Ok(Some(AuthToken::BlindAuth(BlindAuthToken {
            auth_proof: auth_proof.clone().into(),
        })))
    }

    /// Auth for request
    pub async fn get_auth_for_request(
        &self,
        method: &ProtectedEndpoint,
    ) -> Result<Option<AuthToken>, Error> {
        match self.is_protected(method).await {
            Some(auth) => match auth {
                AuthRequired::Clear => Ok(Some(AuthToken::ClearAuth(
                    self.cat
                        .clone()
                        .read()
                        .await
                        .as_ref()
                        .ok_or(Error::CatNotSet)?
                        .clone(),
                ))),
                AuthRequired::Blind => {
                    let proof = self
                        .get_blind_auth_token()
                        .await?
                        .ok_or(Error::InsufficientBlindAuthTokens)?;

                    Ok(Some(proof))
                }
            },
            None => Ok(None),
        }
    }

    /// Mint blind auth
    #[instrument(skip(self))]
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Error> {
        tracing::debug!("Minting {} blind auth proofs", amount);
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let active_keyset_id = self.get_active_mint_blind_auth_keyset().await?.id;

        let premint_secrets =
            PreMintSecrets::random(active_keyset_id, amount, &SplitTarget::Value(1.into()))?;

        let request = MintAuthRequest {
            outputs: premint_secrets.blinded_messages(),
        };

        let mint_res = self.client.post_mint_blind_auth(request).await?;

        let keys = self.get_keyset_keys(active_keyset_id).await?;

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
        Ok(self.get_unspent_auth_proofs().await?.total_amount()?)
    }
}
