use cdk_common::nut22::AuthProofWithoutDleq;
use cdk_common::{CurrencyUnit, MintKeySet};
use tracing::instrument;

use super::nut21::ProtectedEndpoint;
use super::{
    AuthRequired, AuthToken, BlindAuthToken, BlindSignature, BlindedMessage, Error, Id, Mint, State,
};
use crate::dhke::{sign_message, verify_message};
use crate::Amount;

impl Mint {
    /// Check if and what kind of auth is required for a method
    #[instrument(skip(self), fields(endpoint = ?method))]
    pub async fn is_protected(
        &self,
        method: &ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Error> {
        if let Some(auth_db) = self.auth_localstore.as_ref() {
            Ok(auth_db.get_auth_for_endpoint(*method).await?)
        } else {
            Ok(None)
        }
    }

    /// Verify Clear auth
    #[instrument(skip_all, fields(token_len = token.len()))]
    pub async fn verify_clear_auth(&self, token: String) -> Result<(), Error> {
        Ok(self
            .oidc_client
            .as_ref()
            .ok_or(Error::OidcNotSet)?
            .verify_cat(&token)
            .await?)
    }

    /// Ensure Keyset is loaded in mint
    #[instrument(skip(self))]
    pub async fn ensure_blind_auth_keyset_loaded(&self, id: &Id) -> Result<MintKeySet, Error> {
        {
            if let Some(keyset) = self.keysets.read().await.get(id) {
                return Ok(keyset.clone());
            }
        }

        tracing::info!(
            "Keyset {:?} not found in memory, attempting to load from storage",
            id
        );

        let mut keysets = self.keysets.write().await;

        // Get auth_localstore reference
        let auth_localstore = match self.auth_localstore.as_ref() {
            Some(store) => store,
            None => {
                tracing::error!("Auth localstore is not configured");
                return Err(Error::AmountKey);
            }
        };

        // Get keyset info from storage
        let keyset_info = match auth_localstore.get_keyset_info(id).await {
            Ok(Some(info)) => {
                tracing::debug!("Found keyset info in storage for ID {:?}", id);
                info
            }
            Ok(None) => {
                tracing::error!("Keyset with ID {:?} not found in storage", id);
                return Err(Error::KeysetUnknown(*id));
            }
            Err(e) => {
                tracing::error!("Error retrieving keyset info from storage: {:?}", e);
                return Err(e.into());
            }
        };

        let id = keyset_info.id;
        tracing::info!("Generating and inserting keyset {:?} into memory", id);
        let keyset = self.generate_keyset(keyset_info);

        keysets.insert(id, keyset.clone());
        tracing::debug!("Keyset {:?} successfully loaded", id);
        Ok(keyset)
    }

    /// Verify Blind auth
    #[instrument(skip(self, token))]
    pub async fn verify_blind_auth(&self, token: &BlindAuthToken) -> Result<(), Error> {
        let proof = &token.auth_proof;
        let keyset_id = proof.keyset_id;

        tracing::trace!(
            "Starting blind auth verification for keyset ID: {:?}",
            keyset_id
        );

        // Ensure the keyset is loaded
        let keyset = self
            .ensure_blind_auth_keyset_loaded(&keyset_id)
            .await
            .map_err(|err| {
                tracing::error!("Failed to load keyset: {:?}", err);
                err
            })?;

        // Verify keyset is for auth
        if keyset.unit != CurrencyUnit::Auth {
            tracing::warn!(
                "Blind auth attempted with non-auth keyset. Found unit: {:?}",
                keyset.unit
            );
            return Err(Error::BlindAuthFailed);
        }

        // Get the keypair for amount 1
        let keypair = match keyset.keys.get(&Amount::from(1)) {
            Some(key_pair) => key_pair,
            None => {
                tracing::error!("No keypair found for amount 1 in keyset {:?}", keyset_id);
                return Err(Error::AmountKey);
            }
        };

        // Verify the message
        match verify_message(&keypair.secret_key, proof.c, proof.secret.as_bytes()) {
            Ok(_) => {
                tracing::trace!(
                    "Blind signature verification successful for keyset ID: {:?}",
                    keyset_id
                );
            }
            Err(e) => {
                tracing::error!("Blind signature verification failed: {:?}", e);
                return Err(e.into());
            }
        }

        Ok(())
    }

    /// Verify Auth
    ///
    /// If it is a blind auth this will also burn the proof
    #[instrument(skip_all)]
    pub async fn verify_auth(
        &self,
        auth_token: Option<AuthToken>,
        endpoint: &ProtectedEndpoint,
    ) -> Result<(), Error> {
        if let Some(auth_required) = self.is_protected(endpoint).await? {
            tracing::info!(
                "Auth required for endpoint: {:?}, type: {:?}",
                endpoint,
                auth_required
            );

            let auth_token = match auth_token {
                Some(token) => token,
                None => match auth_required {
                    AuthRequired::Clear => {
                        tracing::warn!(
                            "No auth token provided for protected endpoint: {:?}, expected clear auth.",
                            endpoint
                        );
                        return Err(Error::ClearAuthRequired);
                    }
                    AuthRequired::Blind => {
                        tracing::warn!(
                            "No auth token provided for protected endpoint: {:?}, expected blind auth.",
                            endpoint
                        );
                        return Err(Error::BlindAuthRequired);
                    }
                },
            };

            match (auth_required, auth_token) {
                (AuthRequired::Clear, AuthToken::ClearAuth(token)) => {
                    tracing::debug!("Verifying clear auth token");
                    match self.verify_clear_auth(token.clone()).await {
                        Ok(_) => tracing::info!("Clear auth verification successful"),
                        Err(e) => {
                            tracing::error!("Clear auth verification failed: {:?}", e);
                            return Err(e);
                        }
                    }
                }
                (AuthRequired::Blind, AuthToken::BlindAuth(token)) => {
                    tracing::debug!(
                        "Verifying blind auth token with keyset_id: {:?}",
                        token.auth_proof.keyset_id
                    );

                    match self.verify_blind_auth(&token).await {
                        Ok(_) => tracing::debug!("Blind auth signature verification successful"),
                        Err(e) => {
                            tracing::error!("Blind auth verification failed: {:?}", e);
                            return Err(e);
                        }
                    }

                    let auth_proof = token.auth_proof;

                    self.check_blind_auth_proof_spendable(auth_proof)
                        .await
                        .map_err(|err| {
                            tracing::error!("Failed to spend blind auth proof: {:?}", err);
                            err
                        })?;
                }
                (AuthRequired::Blind, other) => {
                    tracing::warn!(
                        "Blind auth required but received different auth type: {:?}",
                        other
                    );
                    return Err(Error::BlindAuthRequired);
                }
                (AuthRequired::Clear, other) => {
                    tracing::warn!(
                        "Clear auth required but received different auth type: {:?}",
                        other
                    );
                    return Err(Error::ClearAuthRequired);
                }
            }
        } else {
            tracing::debug!("No auth required for endpoint: {:?}", endpoint);
        }

        tracing::debug!("Auth verification completed successfully");
        Ok(())
    }

    /// Check state of blind auth proof and mark it as spent
    #[instrument(skip_all)]
    pub async fn check_blind_auth_proof_spendable(
        &self,
        proof: AuthProofWithoutDleq,
    ) -> Result<(), Error> {
        tracing::trace!(
            "Checking if blind auth proof is spendable for keyset ID: {:?}",
            proof.keyset_id
        );

        // Get auth_localstore reference
        let auth_localstore = match self.auth_localstore.as_ref() {
            Some(store) => store,
            None => {
                tracing::error!("Auth localstore is not configured");
                return Err(Error::AmountKey);
            }
        };

        // Calculate the Y value for the proof
        let y = proof.y().map_err(|err| {
            tracing::error!("Failed to calculate Y value for proof: {:?}", err);
            err
        })?;

        // Add proof to the database
        auth_localstore
            .add_proof(proof.clone())
            .await
            .map_err(|err| {
                tracing::error!("Failed to add proof to database: {:?}", err);
                err
            })?;

        // Update proof state to spent
        let state = match auth_localstore.update_proof_state(&y, State::Spent).await {
            Ok(state) => {
                tracing::debug!(
                    "Successfully updated proof state to SPENT, previous state: {:?}",
                    state
                );
                state
            }
            Err(e) => {
                tracing::error!("Failed to update proof state: {:?}", e);
                return Err(e.into());
            }
        };

        // Check previous state
        match state {
            Some(State::Spent) => {
                tracing::warn!("Token already spent: {:?}", y);
                return Err(Error::TokenAlreadySpent);
            }
            Some(State::Pending) => {
                tracing::warn!("Token is pending: {:?}", y);
                return Err(Error::TokenPending);
            }
            Some(other_state) => {
                tracing::trace!("Token was in state {:?}, now marked as spent", other_state);
            }
            None => {
                tracing::trace!("Token was in state None, now marked as spent");
            }
        };

        Ok(())
    }

    /// Blind Sign
    #[instrument(skip_all)]
    pub async fn auth_blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;

        // Ensure the keyset is loaded
        let keyset = match self.ensure_blind_auth_keyset_loaded(keyset_id).await {
            Ok(keyset) => keyset,
            Err(e) => {
                tracing::error!("Failed to load keyset: {:?}", e);
                return Err(e);
            }
        };

        // Get auth_localstore reference
        let auth_localstore = match self.auth_localstore.as_ref() {
            Some(store) => store,
            None => {
                tracing::error!("Auth localstore is not configured");
                return Err(Error::AuthSettingsUndefined);
            }
        };

        // Get keyset info
        let keyset_info = match auth_localstore.get_keyset_info(keyset_id).await {
            Ok(Some(info)) => info,
            Ok(None) => {
                tracing::error!("Keyset with ID {:?} not found in storage", keyset_id);
                return Err(Error::UnknownKeySet);
            }
            Err(e) => {
                tracing::error!("Error retrieving keyset info from storage: {:?}", e);
                return Err(e.into());
            }
        };

        // Get active keyset ID
        let active = match auth_localstore.get_active_keyset_id().await {
            Ok(Some(id)) => id,
            Ok(None) => {
                tracing::error!("No active keyset found");
                return Err(Error::InactiveKeyset);
            }
            Err(e) => {
                tracing::error!("Error retrieving active keyset ID: {:?}", e);
                return Err(e.into());
            }
        };

        // Check that the keyset is active and should be used to sign
        if keyset_info.id.ne(&active) {
            tracing::warn!(
                "Keyset {:?} is not active. Active keyset is {:?}",
                keyset_info.id,
                active
            );
            return Err(Error::InactiveKeyset);
        }

        // Get the keypair for the specified amount
        let key_pair = match keyset.keys.get(amount) {
            Some(key_pair) => key_pair,
            None => {
                tracing::error!(
                    "No keypair found for amount {:?} in keyset {:?}",
                    amount,
                    keyset_id
                );
                return Err(Error::AmountKey);
            }
        };

        // Sign the message
        let c = match sign_message(&key_pair.secret_key, blinded_secret) {
            Ok(signature) => signature,
            Err(e) => {
                tracing::error!("Failed to sign message: {:?}", e);
                return Err(e.into());
            }
        };

        // Create blinded signature
        let blinded_signature = match BlindSignature::new(
            *amount,
            c,
            keyset_info.id,
            &blinded_message.blinded_secret,
            key_pair.secret_key.clone(),
        ) {
            Ok(sig) => sig,
            Err(e) => {
                tracing::error!("Failed to create blinded signature: {:?}", e);
                return Err(e.into());
            }
        };

        tracing::trace!(
            "Blind signing completed successfully for keyset ID: {:?}",
            keyset_id
        );
        Ok(blinded_signature)
    }
}
