mod auth_connector;
mod auth_wallet;

pub use auth_connector::AuthMintConnector;
pub use auth_wallet::AuthWallet;
use cdk_common::{Amount, AuthProof, AuthToken, Proofs};
use tracing::instrument;

use super::Wallet;
use crate::error::Error;

impl Wallet {
    /// Mint blind auth tokens
    #[instrument(skip_all)]
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Error> {
        self.auth_wallet
            .read()
            .await
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .mint_blind_auth(amount)
            .await
    }

    /// Get unspent auth proofs
    #[instrument(skip_all)]
    pub async fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, Error> {
        self.auth_wallet
            .read()
            .await
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .get_unspent_auth_proofs()
            .await
    }

    /// Set Clear Auth Token (CAT) for authentication
    #[instrument(skip_all)]
    pub async fn set_cat(&self, cat: String) -> Result<(), Error> {
        let auth_wallet = self.auth_wallet.read().await;
        if let Some(auth_wallet) = auth_wallet.as_ref() {
            auth_wallet
                .set_auth_token(AuthToken::ClearAuth(cat))
                .await?;
        }
        Ok(())
    }

    /// Set refresh for authentication
    #[instrument(skip_all)]
    pub async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Error> {
        let auth_wallet = self.auth_wallet.read().await;
        if let Some(auth_wallet) = auth_wallet.as_ref() {
            auth_wallet.set_refresh_token(Some(refresh_token)).await;
        }
        Ok(())
    }

    /// Refresh CAT token
    #[instrument(skip(self))]
    pub async fn refresh_access_token(&self) -> Result<(), Error> {
        let auth_wallet = self.auth_wallet.read().await;
        if let Some(auth_wallet) = auth_wallet.as_ref() {
            auth_wallet.refresh_access_token().await?;
        }
        Ok(())
    }
}
