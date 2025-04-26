use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id, MintKeySet, Proof};
use cdk_common::{database, Error};
use tokio::sync::RwLock;

use crate::common::{create_new_keyset, derivation_path_from_unit, init_keysets};
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet};

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
pub struct DbSignatory {
    keysets: RwLock<HashMap<Id, (MintKeySetInfo, MintKeySet)>>,
    active_keysets: RwLock<HashMap<CurrencyUnit, Id>>,
    localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
    auth_localstore:
        Option<Arc<dyn database::MintAuthDatabase<Err = database::Error> + Send + Sync>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    xpriv: Xpriv,
}

impl DbSignatory {
    /// Creates a new MemorySignatory instance
    pub async fn new(
        localstore: Arc<dyn database::MintKeysDatabase<Err = database::Error> + Send + Sync>,
        auth_localstore: Option<
            Arc<dyn database::MintAuthDatabase<Err = database::Error> + Send + Sync>,
        >,
        seed: &[u8],
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let (mut active_keysets, active_keyset_units) = init_keysets(
            xpriv,
            &secp_ctx,
            &localstore,
            &supported_units,
            &custom_paths,
        )
        .await?;

        if let Some(auth_localstore) = auth_localstore.as_ref() {
            tracing::info!("Auth enabled creating auth keysets");
            let derivation_path = match custom_paths.get(&CurrencyUnit::Auth) {
                Some(path) => path.clone(),
                None => derivation_path_from_unit(CurrencyUnit::Auth, 0)
                    .ok_or(Error::UnsupportedUnit)?,
            };

            let (keyset, keyset_info) = create_new_keyset(
                &secp_ctx,
                xpriv,
                derivation_path,
                Some(0),
                CurrencyUnit::Auth,
                1,
                0,
            );

            let id = keyset_info.id;
            auth_localstore.add_keyset_info(keyset_info).await?;
            auth_localstore.set_active_keyset(id).await?;
            active_keysets.insert(id, keyset);
        }

        // Create new keysets for supported units that aren't covered by the current keysets
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

        let keys = Self {
            keysets: Default::default(),
            active_keysets: Default::default(),
            auth_localstore,
            secp_ctx,
            localstore,
            custom_paths,
            xpriv,
        };
        keys.reload_keys_from_db().await?;

        Ok(keys)
    }

    /// Load all the keysets from the database, even if they are not active.
    ///
    /// Since the database is owned by this process, we can load all the keysets in memory, and use
    /// it as the primary source, and the database as the persistence layer.
    ///
    /// Any operation performed with keysets, are done through this trait and never to the database
    /// directly.
    async fn reload_keys_from_db(&self) -> Result<(), Error> {
        let mut keysets = self.keysets.write().await;
        let mut active_keysets = self.active_keysets.write().await;
        keysets.clear();
        active_keysets.clear();

        let db_active_keysets = self.localstore.get_active_keysets().await?;

        for mut info in self.localstore.get_keyset_infos().await? {
            let id = info.id;
            let keyset = self.generate_keyset(&info);
            info.active = db_active_keysets.get(&info.unit) == Some(&info.id);
            if info.active {
                active_keysets.insert(info.unit.clone(), id);
            }
            keysets.insert(id, (info, keyset));
        }

        if let Some(auth_db) = self.auth_localstore.clone() {
            let active_auth_keyset = auth_db.get_active_keyset_id().await?;
            for mut info in auth_db.get_keyset_infos().await? {
                let id = info.id;
                let keyset = self.generate_keyset(&info);
                if info.unit != CurrencyUnit::Auth {
                    continue;
                }
                info.active = active_auth_keyset == Some(info.id);
                tracing::info!("Loading auth key from {} {:?}", id, info);
                if info.active {
                    active_keysets.insert(info.unit.clone(), id);
                }
                keysets.insert(id, (info, keyset));
            }
        }

        Ok(())
    }

    fn generate_keyset(&self, keyset_info: &MintKeySetInfo) -> MintKeySet {
        MintKeySet::generate_from_xpriv(
            &self.secp_ctx,
            self.xpriv,
            keyset_info.max_order,
            keyset_info.unit.clone(),
            keyset_info.derivation_path.clone(),
        )
    }
}

#[async_trait::async_trait]
impl Signatory for DbSignatory {
    async fn blind_sign(&self, blinded_message: BlindedMessage) -> Result<BlindSignature, Error> {
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;

        let keysets = self.keysets.read().await;
        let (info, key) = keysets.get(&keyset_id).ok_or(Error::UnknownKeySet)?;
        if !info.active {
            return Err(Error::InactiveKeyset);
        }

        let key_pair = key.keys.get(&amount).ok_or(Error::UnknownKeySet)?;
        let c = sign_message(&key_pair.secret_key, &blinded_secret)?;

        let blinded_signature = BlindSignature::new(
            amount,
            c,
            keyset_id,
            &blinded_message.blinded_secret,
            key_pair.secret_key.clone(),
        )?;

        Ok(blinded_signature)
    }

    async fn verify_proof(&self, proof: Proof) -> Result<(), Error> {
        let keysets = self.keysets.read().await;
        let (_, key) = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;
        let key_pair = key.keys.get(&proof.amount).ok_or(Error::UnknownKeySet)?;
        verify_message(&key_pair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    async fn keysets(&self) -> Result<Vec<SignatoryKeySet>, Error> {
        Ok(self
            .keysets
            .read()
            .await
            .values()
            .map(|k| k.into())
            .collect::<Vec<_>>())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<MintKeySetInfo, Error> {
        let path_index = if let Some(path_index) = args.derivation_path_index {
            path_index
        } else {
            let current_keyset_id = self
                .localstore
                .get_active_keyset_id(&args.unit)
                .await?
                .ok_or(Error::UnsupportedUnit)?;

            let keyset_info = self
                .localstore
                .get_keyset_info(&current_keyset_id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            keyset_info.derivation_path_index.unwrap_or(1) + 1
        };

        let derivation_path = match self.custom_paths.get(&args.unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(args.unit.clone(), path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let (_, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(path_index),
            args.unit.clone(),
            args.max_order,
            args.input_fee_ppk,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info.clone()).await?;
        self.localstore.set_active_keyset(args.unit, id).await?;

        self.reload_keys_from_db().await?;

        Ok(keyset_info)
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use bitcoin::key::Secp256k1;
    use bitcoin::Network;
    use cashu::{Amount, PublicKey};
    use cdk_common::MintKeySet;

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
