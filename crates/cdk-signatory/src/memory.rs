use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use cdk_common::amount::Amount;
use cdk_common::database::{self, MintDatabase};
use cdk_common::dhke::{sign_message, verify_message};
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::nuts::nut01::MintKeyPair;
use cdk_common::nuts::{
    self, BlindSignature, BlindedMessage, CurrencyUnit, Id, Kind, MintKeySet, Proof,
};
use cdk_common::secret;
use tokio::sync::RwLock;

use crate::common::{create_new_keyset, derivation_path_from_unit, init_keysets};
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet};

/// In-memory Signatory
///
/// This is the default signatory implementation for the mint.
///
/// The private keys and the all key-related data is stored in memory, in the same process, but it
/// is not accessible from the outside.
pub struct Memory {
    keysets: RwLock<HashMap<Id, (MintKeySetInfo, MintKeySet)>>,
    localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
    auth_localstore:
        Option<Arc<dyn database::MintAuthDatabase<Err = database::Error> + Send + Sync>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    xpriv: Xpriv,
}

impl Memory {
    /// Creates a new MemorySignatory instance
    pub async fn new(
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
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

        Ok(Self {
            keysets: RwLock::new(HashMap::new()),
            auth_localstore,
            secp_ctx,
            localstore,
            custom_paths,
            xpriv,
        })
    }
}

impl Memory {
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
        let keyset_info = if let Some(info) = self.localstore.get_keyset_info(id).await? {
            info
        } else {
            let auth_localstore = self.auth_localstore.as_ref().ok_or(Error::UnknownKeySet)?;
            let keyset_info = auth_localstore
                .get_keyset_info(id)
                .await?
                .ok_or(Error::UnknownKeySet)?;

            let active = match auth_localstore.get_active_keyset_id().await {
                Ok(Some(id)) => id,
                Ok(None) => {
                    tracing::error!("No active keyset found");
                    return Err(Error::InactiveKeyset);
                }
                Err(e) => {
                    tracing::error!("Error retrieving active keyset ID: {:?}", e);
                    return Err(e.into());
                }
            };

            // Check that the keyset is active and should be used to sign
            if keyset_info.id.ne(&active) {
                tracing::warn!(
                    "Keyset {:?} is not active. Active keyset is {:?}",
                    keyset_info.id,
                    active
                );
                return Err(Error::InactiveKeyset);
            }

            keyset_info
        };

        if keysets.contains_key(id) {
            return Ok(keyset_info);
        }
        drop(keysets);

        let id = keyset_info.id;
        let mut keysets = self.keysets.write().await;
        keysets.insert(
            id,
            (
                keyset_info.clone(),
                self.generate_keyset(keyset_info.clone()),
            ),
        );
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

        match keyset.1.keys.get(amount) {
            Some(key_pair) => Ok(key_pair.clone()),
            None => Err(Error::AmountKey),
        }
    }
}

#[async_trait::async_trait]
impl Signatory for Memory {
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
            <&secret::Secret as TryInto<nuts::nut10::Secret>>::try_into(&proof.secret)
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

    async fn auth_keysets(&self) -> Result<Option<Vec<SignatoryKeySet>>, Error> {
        let db = if let Some(db) = self.auth_localstore.as_ref() {
            db.clone()
        } else {
            return Ok(None);
        };

        let keyset_id: Id = db
            .get_active_keyset_id()
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        _ = self.load_and_get_keyset(&keyset_id).await?;

        let active_keyset = self
            .keysets
            .read()
            .await
            .get(&keyset_id)
            .ok_or(Error::UnknownKeySet)?
            .into();

        Ok(Some(vec![active_keyset]))
    }

    async fn keysets(&self) -> Result<Vec<SignatoryKeySet>, Error> {
        for (_, id) in self.localstore.get_active_keysets().await? {
            let _ = self.load_and_get_keyset(&id).await?;
        }

        Ok(self
            .keysets
            .read()
            .await
            .values()
            .filter_map(|k| match k.0.active {
                true => Some(k.into()),
                false => None,
            })
            .collect::<Vec<_>>())
    }

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<MintKeySetInfo, Error> {
        let derivation_path = match self.custom_paths.get(&args.unit) {
            Some(path) => path.clone(),
            None => derivation_path_from_unit(args.unit.clone(), args.derivation_path_index)
                .ok_or(Error::UnsupportedUnit)?,
        };

        let (keyset, keyset_info) = create_new_keyset(
            &self.secp_ctx,
            self.xpriv,
            derivation_path,
            Some(args.derivation_path_index),
            args.unit.clone(),
            args.max_order,
            args.input_fee_ppk,
        );
        let id = keyset_info.id;
        self.localstore.add_keyset_info(keyset_info.clone()).await?;
        self.localstore.set_active_keyset(args.unit, id).await?;

        let mut keysets = self.keysets.write().await;
        keysets.insert(id, (keyset_info.clone(), keyset));

        Ok(keyset_info)
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

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
