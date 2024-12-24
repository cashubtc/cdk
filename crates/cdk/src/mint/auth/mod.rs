use tracing::instrument;

use super::nutxx::ProtectedEndpoint;
use super::{AuthProof, AuthRequired, AuthToken, BlindAuthToken, Error, Id, Mint, State};
use crate::dhke::verify_message;
use crate::Amount;

pub mod auth_database;

impl Mint {
    /// Check if and what kind of auth is required for a method
    pub fn protected(&self, method: &ProtectedEndpoint) -> Option<AuthRequired> {
        self.protected_endpoints.get(method).copied()
    }

    /// Verify Clear auth
    pub fn verify_clear_auth(&self, _token: String) -> Result<(), Error> {
        todo!()
    }

    /// Ensure Keyset is loaded in mint
    #[instrument(skip(self))]
    pub async fn ensure_blind_auth_keyset_loaded(&self, id: &Id) -> Result<(), Error> {
        if self.config.load().keysets.contains_key(id) {
            return Ok(());
        }

        let mut keysets = self.config.load().keysets.clone();
        let keyset_info = self
            .auth_localstore
            .as_ref()
            .ok_or(Error::AmountKey)?
            .get_keyset_info(id)
            .await?
            .ok_or(Error::KeysetUnknown(*id))?;

        let id = keyset_info.id;
        keysets.insert(id, self.generate_keyset(keyset_info));
        self.config.set_keysets(keysets);
        Ok(())
    }

    /// Verify Blind auth
    pub async fn verify_blind_auth(&self, token: &BlindAuthToken) -> Result<(), Error> {
        let proof = &token.auth_proof;
        let keyset_id = proof.keyset_id;

        self.ensure_blind_auth_keyset_loaded(&keyset_id).await?;

        let keyset = self
            .config
            .load()
            .keysets
            .get(&keyset_id)
            .ok_or(Error::UnknownKeySet)?
            .clone();

        let keypair = match keyset.keys.get(&Amount::from(1)) {
            Some(key_pair) => key_pair,
            None => return Err(Error::AmountKey),
        };

        verify_message(&keypair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    /// Verify Auth
    ///
    /// If it is a blind auth this will also burn the proof
    pub async fn verify_auth(
        &self,
        auth_token: Option<AuthToken>,
        endpoint: &ProtectedEndpoint,
    ) -> Result<(), Error> {
        if let Some(auth_required) = self.protected(endpoint) {
            let auth_token = auth_token.ok_or(Error::AuthRequired)?;

            match (auth_required, auth_token) {
                (AuthRequired::Clear, AuthToken::ClearAuth(token)) => {
                    self.verify_clear_auth(token)?
                }
                (AuthRequired::Blind, AuthToken::BlindAuth(token)) => {
                    self.verify_blind_auth(&token).await?;

                    let auth_proof = token.auth_proof;

                    self.check_blind_auth_proof_spendable(auth_proof).await?;
                }
                (_, _) => return Err(Error::AuthRequired),
            }
        }

        Ok(())
    }

    /// Check state of blind auth proof and mark it as spent
    #[instrument(skip_all)]
    pub async fn check_blind_auth_proof_spendable(&self, proof: AuthProof) -> Result<(), Error> {
        let auth_localstore = self.auth_localstore.as_ref().ok_or(Error::AmountKey)?;

        auth_localstore.add_proof(proof.clone()).await?;

        let state = auth_localstore
            .update_proof_state(&proof.y()?, State::Spent)
            .await?;

        match state {
            Some(State::Spent) => {
                return Err(Error::TokenAlreadySpent);
            }
            Some(State::Pending) => {
                return Err(Error::TokenPending);
            }
            _ => (),
        };

        Ok(())
    }
}
