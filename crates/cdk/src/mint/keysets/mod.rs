use std::collections::{HashMap, HashSet};

use cdk_common::BlindedMessage;
use cdk_signatory::signatory::{RotateKeyArguments, SignatoryKeySet};
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

mod auth;

/// Newest active keyset (highest derivation index) per unit; older active ones are rotating out.
pub(crate) fn newest_active_keysets(
    keysets: &[SignatoryKeySet],
) -> HashMap<CurrencyUnit, &SignatoryKeySet> {
    let mut newest: HashMap<CurrencyUnit, &SignatoryKeySet> = HashMap::new();
    for keyset in keysets.iter().filter(|k| k.active) {
        newest
            .entry(keyset.unit.clone())
            .and_modify(|cur| {
                if keyset.version > cur.version {
                    *cur = keyset;
                }
            })
            .or_insert(keyset);
    }
    newest
}

/// Whether a newer active keyset of the same unit supersedes this one, ie it is rotating out.
fn is_superseded(keysets: &[SignatoryKeySet], keyset: &SignatoryKeySet) -> bool {
    keyset.active
        && keysets
            .iter()
            .any(|k| k.active && k.unit == keyset.unit && k.version > keyset.version)
}

impl Mint {
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
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
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
                .filter(|k| k.unit != CurrencyUnit::Auth)
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

    /// Generate a new active keyset for the unit.
    ///
    /// The previous keyset is inactivated now if no melt is pending on it, else it
    /// stays active, refusing new melts, until those settle (NUT-02).
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

        // Read the freshest snapshot and store it under the shared lock. The
        // subscription drain task writes the same ArcSwap; serializing the
        // read-then-store here keeps the last write the newest one, so a
        // concurrent rotation's snapshot can never be clobbered by a stale read.
        {
            let _store = self.keyset_store_lock.lock().await;
            let new_keyset = self.signatory.keysets().await?;
            self.keysets.store(new_keyset.keysets.into());
        }

        if let Err(err) = self.retire_drained_keysets().await {
            tracing::warn!("Could not retire rotated keysets: {}", err);
        }

        Ok(result.into())
    }

    /// Whether the keyset is active but superseded by a newer active keyset of its unit.
    pub fn is_keyset_rotating(&self, id: &Id) -> bool {
        let keysets = self.keysets.load();
        keysets
            .iter()
            .find(|k| &k.id == id)
            .is_some_and(|keyset| is_superseded(&keysets, keyset))
    }

    /// Refuse change outputs on a keyset that is rotating out or already inactive.
    pub(crate) fn verify_change_keyset_open(&self, id: &Id) -> Result<(), Error> {
        let keysets = self.keysets.load();
        match keysets.iter().find(|k| &k.id == id) {
            Some(k) if !k.active => Err(Error::InactiveKeyset),
            Some(k) if is_superseded(&keysets, k) => Err(Error::KeysetRotating),
            Some(_) => Ok(()),
            None => Err(Error::UnknownKeySet),
        }
    }

    /// Refuse melt change outputs on a keyset that is rotating out or expires
    /// before the quote does (NUT-02).
    pub(crate) fn verify_melt_change_keysets(
        &self,
        outputs: &[BlindedMessage],
        quote_expiry: u64,
    ) -> Result<(), Error> {
        let ids: HashSet<Id> = outputs.iter().map(|o| o.keyset_id).collect();
        for id in ids {
            let expires_first = quote_expiry != 0
                && self
                    .get_keyset_info(&id)
                    .and_then(|k| k.final_expiry)
                    .is_some_and(|final_expiry| final_expiry < quote_expiry);
            if expires_first || self.is_keyset_rotating(&id) {
                return Err(Error::KeysetRotating);
            }
        }
        Ok(())
    }

    /// Inactivate rotating keysets that no pending melt has change outputs on.
    ///
    /// Returns the inactivated ids.
    #[instrument(skip(self))]
    pub async fn retire_drained_keysets(&self) -> Result<Vec<Id>, Error> {
        let rotating: Vec<Id> = {
            let keysets = self.keysets.load();
            keysets
                .iter()
                .filter(|k| is_superseded(&keysets, k))
                .map(|k| k.id)
                .collect()
        };

        let mut retired = Vec::new();
        for id in rotating {
            if self.localstore.has_pending_melt_outputs(&id).await? {
                continue;
            }
            self.signatory.inactivate_keyset(id).await?;
            tracing::info!("Inactivated drained keyset {}", id);
            retired.push(id);
        }

        if !retired.is_empty() {
            let _store = self.keyset_store_lock.lock().await;
            let new_keyset = self.signatory.keysets().await?;
            self.keysets.store(new_keyset.keysets.into());
        }

        Ok(retired)
    }
}
