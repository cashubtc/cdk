use std::sync::atomic::Ordering;

use cdk_common::nut02::KeySetVersion;
use cdk_signatory::signatory::{RotateKeyArguments, SignatoryKeySet};
use tracing::instrument;

use super::{
    CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse, KeysetResponse, Mint, MintKeySetInfo,
};
use crate::Error;

#[cfg(feature = "auth")]
mod auth;

impl Mint {
    /// Compute the alternate keyset ID (V1 <-> V2)
    fn compute_alternate_id(&self, keyset: &SignatoryKeySet) -> Option<Id> {
        match keyset.id.get_version() {
            KeySetVersion::Version00 => {
                // Current is V1, compute V2
                Some(Id::v2_from_data(
                    &keyset.keys,
                    &keyset.unit,
                    keyset.final_expiry,
                ))
            }
            KeySetVersion::Version01 => {
                // Current is V2, compute V1
                Some(Id::v1_from_keys(&keyset.keys))
            }
        }
    }

    /// Check if V1 keyset IDs should be exposed
    fn should_expose_v1_ids(&self) -> bool {
        self.expose_v1_keyset_ids.load(Ordering::Relaxed)
    }

    /// Enable or disable V1 keyset ID exposure at runtime
    pub fn set_v1_id_exposure(&self, expose: bool) {
        self.expose_v1_keyset_ids.store(expose, Ordering::Relaxed);
        tracing::info!(
            "V1 keyset ID exposure: {}",
            if expose { "enabled" } else { "disabled" }
        );
    }

    /// Retrieve the public keys of a keyset by ID for distribution to wallet clients.
    /// Supports lookup by both native ID and alternate ID (V1/V2) for backward compatibility.
    #[instrument(skip(self))]
    pub fn keyset_pubkeys(&self, keyset_id: &Id) -> Result<KeysResponse, Error> {
        let keysets = self.keysets.load();

        // Try direct lookup first
        if let Some(keyset) = keysets.iter().find(|keyset| &keyset.id == keyset_id) {
            return Ok(KeysResponse {
                keysets: vec![keyset.into()],
            });
        }

        // If not found, try alternate ID lookup
        keysets
            .iter()
            .find(|keyset| {
                if let Some(alternate_id) = self.compute_alternate_id(keyset) {
                    &alternate_id == keyset_id
                } else {
                    false
                }
            })
            .ok_or(Error::UnknownKeySet)
            .map(|keyset| KeysResponse {
                keysets: vec![keyset.into()],
            })
    }

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    #[instrument(skip_all)]
    pub fn pubkeys(&self) -> KeysResponse {
        let mut keysets_vec = Vec::new();
        let expose_v1 = self.should_expose_v1_ids();

        for keyset in self.keysets.load().iter() {
            if !keyset.active || keyset.unit == CurrencyUnit::Auth {
                continue;
            }

            // Always add the keyset with its native ID
            keysets_vec.push(KeySet::from(keyset));

            // Add alternate ID based on version and configuration
            match keyset.id.get_version() {
                KeySetVersion::Version00 => {
                    // Native V1 keyset - always add V2 ID
                    if let Some(v2_id) = self.compute_alternate_id(keyset) {
                        keysets_vec.push(KeySet {
                            id: v2_id,
                            unit: keyset.unit.clone(),
                            keys: keyset.keys.clone(),
                            final_expiry: keyset.final_expiry,
                        });
                    }
                }
                KeySetVersion::Version01 => {
                    // Native V2 keyset - add V1 ID only if enabled
                    if expose_v1 {
                        if let Some(v1_id) = self.compute_alternate_id(keyset) {
                            keysets_vec.push(KeySet {
                                id: v1_id,
                                unit: keyset.unit.clone(),
                                keys: keyset.keys.clone(),
                                final_expiry: keyset.final_expiry,
                            });
                        }
                    }
                }
            }
        }

        KeysResponse {
            keysets: keysets_vec,
        }
    }

    /// Return a list of all supported keysets
    #[instrument(skip_all)]
    pub fn keysets(&self) -> KeysetResponse {
        let mut keysets_vec = Vec::new();
        let expose_v1 = self.should_expose_v1_ids();

        for k in self.keysets.load().iter() {
            if k.unit == CurrencyUnit::Auth {
                continue;
            }

            // Always add with native ID
            keysets_vec.push(KeySetInfo {
                id: k.id,
                unit: k.unit.clone(),
                active: k.active,
                input_fee_ppk: k.input_fee_ppk,
                final_expiry: k.final_expiry,
            });

            // Add alternate ID based on version and configuration
            match k.id.get_version() {
                KeySetVersion::Version00 => {
                    // Native V1 - always add V2 ID
                    if let Some(v2_id) = self.compute_alternate_id(k) {
                        keysets_vec.push(KeySetInfo {
                            id: v2_id,
                            unit: k.unit.clone(),
                            active: k.active,
                            input_fee_ppk: k.input_fee_ppk,
                            final_expiry: k.final_expiry,
                        });
                    }
                }
                KeySetVersion::Version01 => {
                    // Native V2 - add V1 ID only if enabled
                    if expose_v1 {
                        if let Some(v1_id) = self.compute_alternate_id(k) {
                            keysets_vec.push(KeySetInfo {
                                id: v1_id,
                                unit: k.unit.clone(),
                                active: k.active,
                                input_fee_ppk: k.input_fee_ppk,
                                final_expiry: k.final_expiry,
                            });
                        }
                    }
                }
            }
        }

        KeysetResponse {
            keysets: keysets_vec,
        }
    }

    /// Get keysets
    #[instrument(skip(self))]
    pub fn keyset(&self, id: &Id) -> Option<KeySet> {
        // Try direct lookup first
        if let Some(keyset) = self.keysets.load().iter().find(|key| &key.id == id) {
            return Some(keyset.into());
        }

        // If not found, try alternate ID lookup
        self.keysets
            .load()
            .iter()
            .find(|keyset| {
                if let Some(alternate_id) = self.compute_alternate_id(keyset) {
                    &alternate_id == id
                } else {
                    false
                }
            })
            .map(|k| k.into())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[instrument(skip(self))]
    pub async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: u64,
    ) -> Result<MintKeySetInfo, Error> {
        let result = self
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit,
                amounts,
                input_fee_ppk,
            })
            .await?;

        let new_keyset = self.signatory.keysets().await?;
        self.keysets.store(new_keyset.keysets.into());

        Ok(result.into())
    }
}
