mod auth_connector;
mod auth_wallet;

pub use auth_connector::AuthMintConnector;
pub use auth_wallet::AuthWallet;
use cdk_common::{Amount, Proof};

use super::Wallet;
use crate::error::Error;

impl Wallet {
    /// Mint blind auth tokens
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<(), Error> {
        self.auth_wallet
            .read()
            .await
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .mint_blind_auth(amount)
            .await?;

        Ok(())
    }

    /// Get unspent auth proofs
    pub async fn get_unspent_auth_proofs(&self) -> Result<Vec<Proof>, Error> {
        self.auth_wallet
            .read()
            .await
            .as_ref()
            .ok_or(Error::AuthSettingsUndefined)?
            .get_unspent_auth_proofs()
            .await
    }
}
