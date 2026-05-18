use tracing::instrument;

use super::nut21::ProtectedEndpoint;
use super::{
    AuthProof, AuthRequired, AuthToken, BlindAuthToken, BlindSignature, BlindedMessage,
    CurrencyUnit, Error, Mint, State,
};

impl Mint {
    /// Check if and what kind of auth is required for a method
    #[instrument(skip(self), fields(endpoint = ?method))]
    pub async fn is_protected(
        &self,
        method: &ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Error> {
        if let Some(auth_db) = self.auth_localstore.as_ref() {
            if let Some(auth_required) = auth_db.get_auth_for_endpoint(method.clone()).await? {
                return Ok(Some(auth_required));
            }

            Ok(auth_db
                .get_auth_for_endpoints()
                .await?
                .into_iter()
                .filter_map(|(endpoint, auth)| endpoint.match_specificity(method).zip(auth))
                .max_by_key(|(specificity, _)| *specificity)
                .map(|(_, auth)| auth))
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
        let keysets = self.keysets.load();
        let keyset = keysets
            .iter()
            .find(|k| k.id == token.auth_proof.keyset_id)
            .ok_or(Error::UnknownKeySet)?;

        if keyset.unit != CurrencyUnit::Auth {
            return Err(Error::BlindAuthFailed);
        }

        if keyset.is_expired() {
            return Err(Error::ExpiredKeyset);
        }

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

    /// Blind Sign
    #[instrument(skip_all)]
    pub async fn auth_blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        let keyset = self
            .get_keyset_info(&blinded_message.keyset_id)
            .ok_or(Error::UnknownKeySet)?;

        if keyset.unit != CurrencyUnit::Auth {
            return Err(Error::BlindAuthFailed);
        }

        self.signatory
            .blind_sign(vec![blinded_message.to_owned()])
            .await?
            .pop()
            .ok_or(Error::Internal)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk_common::amount::SplitTarget;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{Id, PreMintSecrets};
    use cdk_common::{Amount, CurrencyUnit, PaymentMethod};
    use cdk_fake_wallet::FakeWallet;

    use super::*;
    use crate::mint::{MintBuilder, MintMeltLimits};
    use crate::types::FeeReserve;

    async fn create_auth_enabled_mint() -> Mint {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.expect("mint db"));
        let auth_db = Arc::new(
            cdk_sqlite::mint::MintSqliteAuthDatabase::new(":memory:")
                .await
                .expect("auth db"),
        );

        let mut mint_builder = MintBuilder::new(db.clone());

        let fee_reserve = FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        };
        let ln_fake_backend = FakeWallet::new(
            fee_reserve,
            HashMap::default(),
            HashSet::default(),
            2,
            CurrencyUnit::Sat,
        );

        mint_builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(ln_fake_backend),
            )
            .await
            .expect("payment processor");

        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mint = mint_builder
            .with_auth(
                auth_db,
                "https://example.com/.well-known/openid-configuration".to_string(),
                "test-client".to_string(),
                vec![],
            )
            .with_blind_auth(50, vec![])
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");

        mint.start().await.expect("start mint");
        mint
    }

    fn output_for_keyset(keyset_id: Id) -> BlindedMessage {
        let fee_and_amounts = (0, vec![1]).into();

        PreMintSecrets::random(
            keyset_id,
            Amount::from(1),
            &SplitTarget::Value(1.into()),
            &fee_and_amounts,
        )
        .expect("premint secrets")
        .blinded_messages()
        .pop()
        .expect("blinded message")
    }

    #[tokio::test]
    async fn auth_blind_sign_rejects_non_auth_keysets() {
        let mint = create_auth_enabled_mint().await;
        let active_keysets = mint.get_active_keysets();
        let auth_keyset_id = active_keysets
            .get(&CurrencyUnit::Auth)
            .copied()
            .expect("auth keyset");
        let sat_keyset_id = active_keysets
            .get(&CurrencyUnit::Sat)
            .copied()
            .expect("sat keyset");

        let auth_output = output_for_keyset(auth_keyset_id);
        mint.auth_blind_sign(&auth_output)
            .await
            .expect("auth keyset should sign");

        let sat_output = output_for_keyset(sat_keyset_id);
        let err = mint
            .auth_blind_sign(&sat_output)
            .await
            .expect_err("sat keyset must not sign through auth path");

        assert!(matches!(err, Error::BlindAuthFailed));
    }
}
