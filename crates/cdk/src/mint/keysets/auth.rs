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
        let mut active_keysets = self.localstore.get_active_keysets().await?;

        // We don't want to return auth keys here even though in the db we treat them the same
        active_keysets.retain(|unit, _| unit == &CurrencyUnit::Auth);

        let active_keysets: HashSet<&Id> = active_keysets.values().collect();

        for id in active_keysets.iter() {
            self.ensure_keyset_loaded(id).await?;
        }

        let keysets = self.config.load().keysets.clone();

        Ok(KeysResponse {
            keysets: keysets
                .values()
                .filter(|k| active_keysets.contains(&k.id))
                .map(|k| k.clone().into())
                .collect(),
        })
    }

    /// Return a list of auth keysets
    #[instrument(skip_all)]
    pub async fn auth_keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;
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
