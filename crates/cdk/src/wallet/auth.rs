use tracing::instrument;

use super::Wallet;
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nutxx1::MintAuthRequest;
use crate::nuts::{
    AuthRequired, AuthToken, BlindAuthToken, CurrencyUnit, KeySetInfo, PreMintSecrets, Proofs,
    ProtectedEndpoint, State,
};
use crate::types::ProofInfo;
use crate::{Amount, Error};

impl Wallet {
    /// Get active keyset for mint
    ///
    /// Queries mint for current keysets then gets [`Keys`] for any unknown
    /// keysets
    #[instrument(skip(self))]
    pub async fn get_active_mint_blind_auth_keysets(&self) -> Result<Vec<KeySetInfo>, Error> {
        let keysets = self.client.get_mint_keysets(None).await?;
        let keysets = keysets.keysets;

        self.localstore
            .add_mint_keysets(self.mint_url.clone(), keysets.clone())
            .await?;

        let active_keysets = keysets
            .clone()
            .into_iter()
            .filter(|k| k.active && k.unit == CurrencyUnit::Auth)
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
                Some(self.unit.clone()),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Check if and what kind of auth is required for a method
    pub fn protected(&self, method: &ProtectedEndpoint) -> Option<AuthRequired> {
        self.protected_endpoints.get(method).copied()
    }

    /// Get Auth Token
    pub async fn get_blind_auth_token(&self) -> Result<Option<AuthToken>, Error> {
        let unspent = self.get_unspent_auth_proofs().await?;

        let auth_proof = match unspent.first() {
            Some(proof) => proof,
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
        match self.protected_endpoints.get(method) {
            Some(auth) => match auth {
                AuthRequired::Clear => Ok(Some(AuthToken::ClearAuth(
                    self.cat.clone().ok_or(Error::AuthRequired)?,
                ))),
                AuthRequired::Blind => {
                    let proof = self
                        .get_blind_auth_token()
                        .await?
                        .ok_or(Error::InsufficientFunds)?;
                    Ok(Some(proof))
                }
            },
            None => Ok(None),
        }
    }

    /// Mint blind auth
    #[instrument(skip(self))]
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Amount, Error> {
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

        let proofs = proofs
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

        let proof_count = proofs.len();

        // Add new proofs to store
        self.localstore.update_proofs(proofs, vec![]).await?;

        Ok(Amount::from(proof_count as u64))
    }

    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_blind_auth_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_unspent_auth_proofs().await?.total_amount()?)
    }
}
