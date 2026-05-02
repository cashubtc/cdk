use cdk_signatory::signatory::RotateKeyArguments;
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

mod auth;

impl Mint {
    /// Returns true if the given keyset should be listed by the public
    /// NUT-01/NUT-02 list endpoints (`GET /v1/keys`, `GET /v1/keysets`).
    ///
    /// Per-ID lookups (`GET /v1/keys/{id}`) intentionally do NOT apply this
    /// filter: wallets holding a conditional token must still be able to
    /// fetch the keys for that specific keyset by ID (see `nuts/CTF.md`).
    ///
    /// Currently this hides from the list endpoints:
    /// - auth keysets
    /// - NUT-CTF conditional keysets (feature-gated), which are only
    ///   enumerated via the NUT-CTF conditional endpoints
    #[inline]
    fn is_listable_keyset(keyset: &cdk_signatory::signatory::SignatoryKeySet) -> bool {
        if keyset.unit == CurrencyUnit::Auth {
            return false;
        }
        #[cfg(feature = "conditional-tokens")]
        if keyset.condition_id.is_some() {
            return false;
        }
        true
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip(self))]
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        self.keysets
            .load()
            .iter()
            .find(|keyset| &keyset.id == keyset_id)
            .ok_or(Error::UnknownKeySet)
            .map(|key| KeysResponse {
                keysets: vec![key.into()],
            })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub fn pubkeys(&self) -> KeysResponse {
        KeysResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|keyset| keyset.active && Self::is_listable_keyset(keyset))
                .map(|key| key.into())
                .collect::<Vec<_>>(),
        }
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub fn keysets(&self) -> KeysetResponse {
        KeysetResponse {
            keysets: self
                .keysets
                .load()
                .iter()
                .filter(|k| Self::is_listable_keyset(k))
                .map(|k| KeySetInfo {
                    id: k.id,
                    unit: k.unit.clone(),
                    active: k.active,
                    input_fee_ppk: k.input_fee_ppk,
                    final_expiry: k.final_expiry,
                })
                .collect(),
        }
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub fn keyset(&self, id: &Id) -> Option<KeySet> {
        self.keysets
            .load()
            .iter()
            .find(|key| &key.id == id)
            .map(|x| x.into())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: u64,
        use_keyset_v2: bool,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Error> {
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                amounts,
                input_fee_ppk,
                keyset_id_type: if use_keyset_v2 {
                    cdk_common::nut02::KeySetVersion::Version01
                } else {
                    cdk_common::nut02::KeySetVersion::Version00
                },
                final_expiry,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.keysets.into());

        Ok(result.into())
    }
}
