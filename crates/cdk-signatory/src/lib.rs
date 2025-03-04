//! In memory signatory
//!
//! Implements the Signatory trait from cdk-common to manage the key in-process, to be included
//! inside the mint to be executed as a single process.
//!
//! Even if it is embedded in the same process, the keys are not accessible from the outside of this
//! module, all communication is done through the Signatory trait and the signatory manager.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::amount::Amount;
use cdk_common::database::{self, MintDatabase};
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::nut01::MintKeyPair;
use cdk_common::nuts::{
    self, BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, KeySetInfo, KeysResponse,
    KeysetResponse, Kind, MintKeySet, Proof,
};
use cdk_common::secret;
use cdk_common::signatory::{KeysetIdentifier, Signatory};
use cdk_common::util::unix_time;
use tokio::sync::RwLock;

#[cfg(feature = "grpc")]
pub mod proto;

#[cfg(feature = "grpc")]
pub use proto::client::RemoteSigner;

/// Generate new [`MintKeySetInfo`] from path
#[tracing::instrument(skip_all)]
fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    derivation_path_index: Option<u32>,
    unit: CurrencyUnit,
    max_order: u8,
    input_fee_ppk: u64,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        max_order,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        derivation_path_index,
        max_order,
        input_fee_ppk,
    };
    (keyset, keyset_info)
}

fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = unit.derivation_index()?;

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
pub struct MemorySignatory {
    keysets: RwLock<HashMap<Id, MintKeySet>>,
    localstore: Arc<dyn MintDatabase<Err = database::Error> + Send + Sync>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: Xpriv,
}

impl MemorySignatory {
    /// Creates a new MemorySignatory instance
    pub async fn new(
        localstore: Arc<dyn MintDatabase<Err = database::Error> + Send + Sync>,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let mut active_keysets = HashMap::new();
        let keysets_infos = localstore.get_keyset_infos().await?;
        let mut active_keyset_units = vec![];

        if !keysets_infos.is_empty() {
            tracing::debug!("Setting all saved keysets to inactive");
            for keyset in keysets_infos.clone() {
                // Set all to in active
                let mut keyset = keyset;
                keyset.active = false;
                localstore.add_keyset_info(keyset).await?;
            }

            let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
                keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
                    acc.entry(ks.unit.clone()).or_default().push(ks.clone());
                    acc
                });

            for (unit, keysets) in keysets_by_unit {
                let mut keysets = keysets;
                keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

                let highest_index_keyset = keysets
                    .first()
                    .cloned()
                    .expect("unit will not be added to hashmap if empty");

                let keysets: Vec<MintKeySetInfo> = keysets
                    .into_iter()
                    .filter(|ks| ks.derivation_path_index.is_some())
                    .collect();

                if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
                    let derivation_path_index = if keysets.is_empty() {
                        1
                    } else if &highest_index_keyset.input_fee_ppk == input_fee_ppk
                        && &highest_index_keyset.max_order == max_order
                    {
                        let id = highest_index_keyset.id;
                        let keyset = MintKeySet::generate_from_xpriv(
                            &secp_ctx,
                            xpriv,
                            highest_index_keyset.max_order,
                            highest_index_keyset.unit.clone(),
                            highest_index_keyset.derivation_path.clone(),
                        );
                        active_keysets.insert(id, keyset);
                        let mut keyset_info = highest_index_keyset;
                        keyset_info.active = true;
                        localstore.add_keyset_info(keyset_info).await?;
                        localstore.set_active_keyset(unit, id).await?;
                        continue;
                    } else {
                        highest_index_keyset.derivation_path_index.unwrap_or(0) + 1
                    };

                    let derivation_path = match custom_paths.get(&unit) {
                        Some(path) => path.clone(),
                        None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                            .ok_or(Error::UnsupportedUnit)?,
                    };

                    let (keyset, keyset_info) = create_new_keyset(
                        &secp_ctx,
                        xpriv,
                        derivation_path,
                        Some(derivation_path_index),
                        unit.clone(),
                        *max_order,
                        *input_fee_ppk,
                    );

                    let id = keyset_info.id;
                    localstore.add_keyset_info(keyset_info).await?;
                    localstore.set_active_keyset(unit.clone(), id).await?;
                    active_keysets.insert(id, keyset);
                    active_keyset_units.push(unit.clone());
                }
            }
        }

        for (unit, (fee, max_order)) in supported_units {
            if !active_keyset_units.contains(&unit) {
                let derivation_path = match custom_paths.get(&unit) {
                    Some(path) => path.clone(),
                    None => {
                        derivation_path_from_unit(unit.clone(), 0).ok_or(Error::UnsupportedUnit)?
                    }
                };

                let (keyset, keyset_info) = create_new_keyset(
                    &secp_ctx,
                    xpriv,
                    derivation_path,
                    Some(0),
                    unit.clone(),
                    max_order,
                    fee,
                );

                let id = keyset_info.id;
                localstore.add_keyset_info(keyset_info).await?;
                localstore.set_active_keyset(unit, id).await?;
                active_keysets.insert(id, keyset);
            }
        }

        Ok(Self {
            keysets: RwLock::new(HashMap::new()),
            secp_ctx,
            localstore,
            xpriv,
        })
    }
}

impl MemorySignatory {
    fn generate_keyset(&self, keyset_info: MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            keyset_info.max_order,
            keyset_info.unit,
            keyset_info.derivation_path,
        )
    }

    async fn load_and_get_keyset(&self, id: &Id) -> Result<MintKeySetInfo, Error> {
        let keysets = self.keysets.read().await;
        let keyset_info = self
            .localstore
            .get_keyset_info(id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        if keysets.contains_key(id) {
            return Ok(keyset_info);
        }
        drop(keysets);

        let id = keyset_info.id;
        let mut keysets = self.keysets.write().await;
        keysets.insert(id, self.generate_keyset(keyset_info.clone()));
        Ok(keyset_info)
    }

    #[tracing::instrument(skip(self))]
    async fn get_keypair_for_amount(
        &self,
        keyset_id: &Id,
        amount: &Amount,
    ) -> Result<MintKeyPair, Error> {
        let keyset_info = self.load_and_get_keyset(keyset_id).await?;
        let active = self
            .localstore
            .get_active_keyset_id(&keyset_info.unit)
            .await?
            .ok_or(Error::InactiveKeyset)?;

        // Check that the keyset is active and should be used to sign
        if keyset_info.id != active {
            return Err(Error::InactiveKeyset);
        }

        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?;

        match keyset.keys.get(amount) {
            Some(key_pair) => Ok(key_pair.clone()),
            None => Err(Error::AmountKey),
        }
    }
}

#[async_trait::async_trait]
impl Signatory for MemorySignatory {
    async fn blind_sign(&self, blinded_message: BlindedMessage) -> Result<BlindSignature, Error> {
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;
        let key_pair = self.get_keypair_for_amount(&keyset_id, &amount).await?;
        let c = sign_message(&key_pair.secret_key, &blinded_secret)?;

        let blinded_signature = BlindSignature::new(
            amount,
            c,
            keyset_id,
            &blinded_message.blinded_secret,
            key_pair.secret_key,
        )?;

        Ok(blinded_signature)
    }

    async fn verify_proof(&self, proof: Proof) -> Result<(), Error> {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) =
            <&crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(&proof.secret)
        {
            // Checks and verifies known secret kinds.
            // If it is an unknown secret kind it will be treated as a normal secret.
            // Spending conditions will **not** be check. It is up to the wallet to ensure
            // only supported secret kinds are used as there is no way for the mint to
            // enforce only signing supported secrets as they are blinded at
            // that point.
            match secret.kind {
                Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
        }

        let key_pair = self
            .get_keypair_for_amount(&proof.keyset_id, &proof.amount)
            .await?;

        verify_message(&key_pair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    async fn keyset(&self, keyset_id: Id) -> Result<Option<KeySet>, Error> {
        self.load_and_get_keyset(&keyset_id).await?;
        Ok(self
            .keysets
            .read()
            .await
            .get(&keyset_id)
            .map(|k| k.clone().into()))
    }

    async fn keyset_pubkeys(&self, keyset_id: Id) -> Result<KeysResponse, Error> {
        self.load_and_get_keyset(&keyset_id).await?;
        Ok(KeysResponse {
            keysets: vec![self
                .keysets
                .read()
                .await
                .get(&keyset_id)
                .ok_or(Error::UnknownKeySet)?
                .clone()
                .into()],
        })
    }

    async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        let active_keysets = self.localstore.get_active_keysets().await?;
        let active_keysets: HashSet<&Id> = active_keysets.values().collect();
        for id in active_keysets.iter() {
            let _ = self.load_and_get_keyset(id).await?;
        }
        let keysets = self.keysets.read().await;
        Ok(KeysResponse {
            keysets: keysets
                .values()
                .filter_map(|k| match active_keysets.contains(&k.id) {
                    true => Some(k.clone().into()),
                    false => None,
                })
                .collect(),
        })
    }

    async fn keysets(&self) -> Result<KeysetResponse, Error> {
        let keysets = self.localstore.get_keyset_infos().await?;
        let active_keysets: HashSet<Id> = self
            .localstore
            .get_active_keysets()
            .await?
            .values()
            .cloned()
            .collect();

        Ok(KeysetResponse {
            keysets: keysets
                .into_iter()
                .map(|k| KeySetInfo {
                    id: k.id,
                    unit: k.unit,
                    active: active_keysets.contains(&k.id),
                    input_fee_ppk: k.input_fee_ppk,
                })
                .collect(),
        })
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<MintKeySetInfo, Error> {
        let derivation_path = match custom_paths.get(&unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(derivation_path_index),
            unit.clone(),
            max_order,
            input_fee_ppk,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info.clone()).await?;
        self.localstore.set_active_keyset(unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, keyset);

        Ok(keyset_info)
    }

    async fn get_keyset_info(&self, keyset_id: KeysetIdentifier) -> Result<MintKeySetInfo, Error> {
        let keyset_id = match keyset_id {
            KeysetIdentifier::Id(id) => id,
            KeysetIdentifier::Unit(unit) => self
                .localstore
                .get_active_keyset_id(&unit)
                .await?
                .ok_or(Error::UnsupportedUnit)?,
        };

        self.localstore
            .get_keyset_info(&keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)
    }
}

#[cfg(test)]
mod test {
    use bitcoin::key::Secp256k1;
    use bitcoin::Network;
    use cdk_common::MintKeySet;
    use nuts::PublicKey;

    use super::*;

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = "test_seed".as_bytes();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            seed,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_mod_generate_keyset_from_xpriv() {
        let seed = "test_seed".as_bytes();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }
}
