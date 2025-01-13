use std::collections::HashMap;

use bitcoin::bip32::DerivationPath;
use tracing::instrument;

use super::{CurrencyUnit, Id, KeySet, KeysResponse, KeysetResponse, Mint};
use crate::Error;

impl Mint {
    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub async fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.signatory.keyset_pubkeys(keyset_id.to_owned()).await
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        self.signatory.pubkeys().await
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub async fn keysets(&self) -> Result<KeysetResponse, Error> {
        self.signatory.keysets().await
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub async fn keyset(&self, id: &Id) -> Result<Option<KeySet>, Error> {
        self.signatory.keyset(id.to_owned()).await
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<(), Error> {
        self.signatory
            .rotate_keyset(
                unit,
                derivation_path_index,
                max_order,
                input_fee_ppk,
                custom_paths,
            )
            .await
    }
}
