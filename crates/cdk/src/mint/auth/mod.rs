use tracing::instrument;

use super::nut21::ProtectedEndpoint;
use super::{
    AuthProof, AuthRequired, AuthToken, BlindAuthToken, BlindSignature, BlindedMessage,
    CheckBlindAuthStateRequest, CheckBlindAuthStateResponse, Error, Mint, ProofState,
    SpendBlindAuthRequest, SpendBlindAuthResponse, State,
};

impl Mint {
    /// Check if and what kind of auth is required for a method
    #[instrument(skip(self), fields(endpoint = ?method))]
    pub async fn is_protected(
        &self,
        method: &ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Error> {
        if let Some(auth_db) = self.auth_localstore.as_ref() {
            Ok(auth_db.get_auth_for_endpoint(method.clone()).await?)
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

    /// Verify Blind auth
    #[instrument(skip(self, token))]
    pub async fn verify_blind_auth(&self, token: &BlindAuthToken) -> Result<(), Error> {
        self.signatory
            .verify_proofs(vec![token.auth_proof.clone().into()])
            .await
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
        let auth_required = if let Some(auth_required) = self.is_protected(endpoint).await? {
            tracing::trace!(
                "Auth required for endpoint: {:?}, type: {:?}",
                endpoint,
                auth_required
            );
            auth_required
        } else {
            tracing::trace!("No auth required for endpoint: {:?}", endpoint);
            return Ok(());
        };

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

        tracing::debug!("Auth verification completed successfully");
        Ok(())
    }

    /// Check state of blind auth proof and mark it as spent
    #[instrument(skip_all)]
    pub async fn check_blind_auth_proof_spendable(&self, proof: AuthProof) -> Result<(), Error> {
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

        let mut tx = auth_localstore.begin_transaction().await?;

        // Add proof to the database
        tx.add_proof(proof.clone()).await.map_err(|err| {
            tracing::error!("Failed to add proof to database: {:?}", err);
            err
        })?;

        // Update proof state to spent
        let state = match tx.update_proof_state(&y, State::Spent).await {
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

        tx.commit().await?;

        Ok(())
    }

    /// Get blind auth states without marking as spent
    ///
    /// This method only checks the state of auth proofs without modifying them.
    /// Use this for external verification when you need to check if a BAT is valid
    /// before deciding to accept it.
    #[instrument(skip_all)]
    pub async fn get_blind_auth_states(
        &self,
        request: CheckBlindAuthStateRequest,
    ) -> Result<CheckBlindAuthStateResponse, Error> {
        tracing::debug!(
            "Checking blind auth states for {} proofs",
            request.auth_proofs.len()
        );

        let auth_localstore = self.auth_localstore.as_ref().ok_or_else(|| {
            tracing::error!("Auth localstore is not configured");
            Error::AmountKey
        })?;

        let mut states = Vec::with_capacity(request.auth_proofs.len());

        for auth_proof in &request.auth_proofs {
            // Verify the signature first
            let blind_auth_token = BlindAuthToken::new(auth_proof.clone());
            if let Err(e) = self.verify_blind_auth(&blind_auth_token).await {
                tracing::warn!("Invalid signature for auth proof: {:?}", e);
                return Err(e);
            }

            // Calculate Y value
            let y = auth_proof.y().map_err(|err| {
                tracing::error!("Failed to calculate Y value for proof: {:?}", err);
                err
            })?;

            // Get current state from database
            let proof_states = auth_localstore.get_proofs_states(&[y]).await?;
            let state = proof_states
                .first()
                .cloned()
                .flatten()
                .unwrap_or(State::Unspent);

            states.push(ProofState {
                y,
                state,
                witness: None,
            });
        }

        tracing::debug!("Returning {} proof states", states.len());
        Ok(CheckBlindAuthStateResponse { states })
    }

    /// Spend a blind auth proof
    ///
    /// This method verifies the signature and marks the proof as spent.
    /// Use this when an external app wants to consume a BAT after successful
    /// request processing.
    #[instrument(skip_all)]
    pub async fn spend_blind_auth(
        &self,
        request: SpendBlindAuthRequest,
    ) -> Result<SpendBlindAuthResponse, Error> {
        tracing::debug!(
            "Spending blind auth proof with keyset_id: {:?}",
            request.auth_proof.keyset_id
        );

        // Verify the signature
        let blind_auth_token = BlindAuthToken::new(request.auth_proof.clone());
        self.verify_blind_auth(&blind_auth_token)
            .await
            .map_err(|e| {
                tracing::error!("Blind auth signature verification failed: {:?}", e);
                e
            })?;

        // Calculate Y value
        let y = request.auth_proof.y().map_err(|err| {
            tracing::error!("Failed to calculate Y value for proof: {:?}", err);
            err
        })?;

        // Mark as spent (reuse existing logic)
        self.check_blind_auth_proof_spendable(request.auth_proof)
            .await?;

        tracing::info!("Successfully spent blind auth proof");
        Ok(SpendBlindAuthResponse {
            state: ProofState {
                y,
                state: State::Spent,
                witness: None,
            },
        })
    }

    /// Blind Sign
    #[instrument(skip_all)]
    pub async fn auth_blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        self.signatory
            .blind_sign(vec![blinded_message.to_owned()])
            .await?
            .pop()
            .ok_or(Error::Internal)
    }
}
