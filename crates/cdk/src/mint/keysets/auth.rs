//! Auth keyset functions

use cdk_common::CurrencyUnit;
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
            .keysets()
            .await?
            .into_iter()
            .find(|key| key.info.unit == CurrencyUnit::Auth)
            .ok_or(Error::NoActiveKeyset)?;

        Ok(KeysResponse {
            keysets: vec![key.key],
        })
    }

    /// Return a list of auth keysets
    #[instrument(skip_all)]
    pub async fn auth_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(KeysetResponse {
            keysets: self
                .signatory
                .keysets()
                .await?
                .into_iter()
                .filter_map(|key| {
                    if key.info.unit == CurrencyUnit::Auth {
                        Some(key.info.into())
                    } else {
                        None
                    }
                })
                .collect(),
        })
    }
}
