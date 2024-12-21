//! Auth keyset functions
use std::collections::HashSet;

use tracing::instrument;

use crate::mint::{CurrencyUnit, Id, KeySetInfo, KeysResponse, KeysetResponse};
use crate::{Error, Mint};

impl Mint {
    /// Retrieve the auth public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn auth_pubkeys(&self) -> Result<KeysResponse, Error> {
        let active_keyset_id = self
            .auth_localstore
            .as_ref()
            .ok_or(Error::AuthLocalstoreUndefined)?
            .get_active_keyset_id()
            .await?
            .ok_or(Error::AmountKey)?;

        self.ensure_blind_auth_keyset_loaded(&active_keyset_id)
            .await?;

        let keysets = self.keysets.read().await;

        Ok(KeysResponse {
            keysets: vec![keysets
                .get(&active_keyset_id)
                .ok_or(Error::KeysetUnknown(active_keyset_id))?
                .clone()
                .into()],
        })
    }

    /// Return a list of auth keysets
    #[instrument(skip_all)]
    pub async fn auth_keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self
            .auth_localstore
            .clone()
            .unwrap()
            .get_keyset_infos()
            .await?;
        let active_keysets: HashSet<Id> = self
            .localstore
            .get_active_keysets()
            .await?
            .values()
            .cloned()
            .collect();

        let keysets = keysets
            .into_iter()
            .filter(|k| k.unit == CurrencyUnit::Auth)
            .map(|k| KeySetInfo {
                id: k.id,
                unit: k.unit,
                active: active_keysets.contains(&k.id),
                input_fee_ppk: k.input_fee_ppk,
            })
            .collect();

        Ok(KeysetResponse { keysets })
    }
}
