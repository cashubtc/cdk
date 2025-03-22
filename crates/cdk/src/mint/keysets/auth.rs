//! Auth keyset functions

use tracing::instrument;

use crate::mint::{KeysResponse, KeysetResponse};
use crate::{Error, Mint};

impl Mint {
    /// Retrieve the auth public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn auth_pubkeys(&self) -> Result<KeysResponse, Error> {
        let key = self
            .signatory
            .auth_keysets()
            .await?
            .ok_or(Error::AuthLocalstoreUndefined)?
            .pop()
            .ok_or(Error::AuthLocalstoreUndefined)?;

        Ok(KeysResponse {
            keysets: vec![key.key],
        })
    }

    /// Return a list of auth keysets
    #[instrument(skip_all)]
    pub async fn auth_keysets(&self) -> Result<KeysetResponse, Error> {
        self.signatory
            .auth_keysets()
            .await?
            .map(|all_keysets| KeysetResponse {
                keysets: all_keysets.into_iter().map(|k| k.info.into()).collect(),
            })
            .ok_or(Error::AuthLocalstoreUndefined)
    }
}
