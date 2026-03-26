//! This module provides deterministic public key generation.
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::wallet::P2PKSigningKey;
use cdk_common::{PublicKey, SECP256K1};

use crate::error::Error;

const P2PK_PURPOSE: u32 = 129373;
const P2PK_ACCOUNT: u32 = 10;

/// Generates and stores public key in database
pub async fn generate_public_key(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    seed: &[u8; 64],
) -> Result<PublicKey, Error> {
    let public_keys = localstore.list_p2pk_keys().await?;

    let mut last_derivation_index = 0;

    for public_key in public_keys {
        if public_key.derivation_index >= last_derivation_index {
            last_derivation_index = public_key.derivation_index + 1;
        }
    }

    let derivation_path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(P2PK_PURPOSE)?,
        ChildNumber::from_hardened_idx(P2PK_ACCOUNT)?,
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_normal_idx(last_derivation_index)?,
    ]);

    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;

    let derived_key = xpriv.derive_priv(&SECP256K1, &derivation_path)?.private_key;
    let pubkey = PublicKey::from(derived_key.public_key(&SECP256K1));

    localstore
        .add_p2pk_key(&pubkey, derivation_path, last_derivation_index)
        .await?;
    Ok(pubkey)
}

/// Gets public key by its hex value
pub async fn get_public_key(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    pubkey: &PublicKey,
) -> Result<Option<P2PKSigningKey>, database::Error> {
    localstore.get_p2pk_key(pubkey).await
}

/// Gets list of stored public keys in database
pub async fn get_public_keys(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
) -> Result<Vec<P2PKSigningKey>, database::Error> {
    localstore.list_p2pk_keys().await
}

/// Gets the latest generated P2PK signing key (most recently created)
pub async fn get_latest_public_key(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
) -> Result<Option<P2PKSigningKey>, database::Error> {
    localstore.latest_p2pk().await
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use bip39::Mnemonic;
    use cdk_common::database::WalletDatabase;

    use super::*;

    #[tokio::test]
    async fn nut13_test_vector() {
        let localstore: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
            Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::from_str(
            "half depart obvious quality work element tank gorilla view sugar picture humble",
        )
        .unwrap();

        let seed = mnemonic.to_seed_normalized("");

        let pubkey = generate_public_key(&localstore, &seed).await.unwrap();
        let pubkey_1 = generate_public_key(&localstore, &seed).await.unwrap();
        let pubkey_2 = generate_public_key(&localstore, &seed).await.unwrap();
        let pubkey_3 = generate_public_key(&localstore, &seed).await.unwrap();
        let pubkey_4 = generate_public_key(&localstore, &seed).await.unwrap();

        assert_eq!(
            pubkey.to_hex(),
            "021693d45f4fdf610ae641fedb0944fb460fbb8264f21c19d2626c3da755fcbbcb".to_string()
        );
        assert_eq!(
            pubkey_1.to_hex(),
            "0395461ab678058c0ed6aa39f38dda490eaa163e9ad27070b23ec3d06b41e07535".to_string()
        );
        assert_eq!(
            pubkey_2.to_hex(),
            "02a05e4e593a633e9b4405f01c9632c8afde24cb613017a1aee56fd76291ad26d1".to_string()
        );
        assert_eq!(
            pubkey_3.to_hex(),
            "033addea25c3873b93d67d536c61c9d9c993f6efd8b9dfa657951b66b5001e51dd".to_string()
        );
        assert_eq!(
            pubkey_4.to_hex(),
            "03c964bdf42fc82b6c574615746eeca37527a24f1fdfc1b34a732c53843b5744a5".to_string()
        );
        let stored_keys = localstore.list_p2pk_keys().await.unwrap();
        assert_eq!(
            stored_keys[0].derivation_path.to_string(),
            "129373'/10'/0'/0'/4"
        );
        assert_eq!(
            stored_keys[1].derivation_path.to_string(),
            "129373'/10'/0'/0'/3"
        );
        assert_eq!(
            stored_keys[2].derivation_path.to_string(),
            "129373'/10'/0'/0'/2"
        );
        assert_eq!(
            stored_keys[3].derivation_path.to_string(),
            "129373'/10'/0'/0'/1"
        );
        assert_eq!(
            stored_keys[4].derivation_path.to_string(),
            "129373'/10'/0'/0'/0"
        );
    }
}
