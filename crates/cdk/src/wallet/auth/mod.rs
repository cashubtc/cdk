mod auth_connector;
mod auth_wallet;

use std::sync::Arc;

pub use auth_connector::AuthMintConnector;
pub use auth_wallet::AuthWallet;
use cdk_common::{Amount, AuthToken, MintInfo, Proof};

use super::{HttpClient, Wallet};
use crate::error::Error;

impl Wallet {
    /// Add auth wallet to wallet
    pub async fn add_auth_wallet(
        &self,
        cat: String,
        mint_info: Option<MintInfo>,
    ) -> Result<Self, Error> {
        let mint_info = match mint_info {
            Some(mint_info) => mint_info,
            None => self
                .get_mint_info()
                .await?
                .ok_or(Error::CouldNotGetMintInfo)?,
        };

        let auth_wallet = AuthWallet::new(
            self.mint_url.clone(),
            AuthToken::ClearAuth(cat),
            self.localstore.clone(),
            mint_info.protected_endpoints(),
        );
        let http_client = Arc::new(HttpClient::new(
            self.mint_url.clone(),
            Some(auth_wallet.clone()),
        ));

        Ok(Self {
            client: http_client,
            auth_wallet: Some(auth_wallet),
            ..self.clone()
        })
    }

    /// Mint blind auth tokens
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<(), Error> {
        self.auth_wallet
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .mint_blind_auth(amount)
            .await?;

        Ok(())
    }

    /// Get unspent auth proofs
    pub async fn get_unspent_auth_proofs(&self) -> Result<Vec<Proof>, Error> {
        self.auth_wallet
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .get_unspent_auth_proofs()
            .await
    }
}
