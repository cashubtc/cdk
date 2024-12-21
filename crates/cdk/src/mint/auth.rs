use super::nutxx::ProtectedEndpoint;
use super::{AuthRequired, AuthToken, BlindAuthToken, Error, Mint, State};
use crate::dhke::verify_message;
use crate::Amount;
impl Mint {
    /// Check if and what kind of auth is required for a method
    pub fn protected(&self, method: &ProtectedEndpoint) -> Option<AuthRequired> {
        self.protected_endpoints.get(method).copied()
    }

    /// Verify Clear auth
    pub fn verify_clear_auth(&self, _token: String) -> Result<(), Error> {
        todo!()
    }

    /// Verify Blind auth
    pub async fn verify_blind_auth(&self, token: &BlindAuthToken) -> Result<(), Error> {
        let proof = &token.auth_proof;
        let keyset_id = proof.keyset_id;

        self.ensure_keyset_loaded(&keyset_id).await?;

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
                    let y = auth_proof.y()?;

                    self.localstore
                        .add_proofs(vec![auth_proof.into()], None)
                        .await?;
                    self.check_ys_spendable(&[y], State::Spent).await?;
                }
                (_, _) => return Err(Error::AuthRequired),
            }
        }

        Ok(())
    }
}
