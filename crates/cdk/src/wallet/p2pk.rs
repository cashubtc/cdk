//! This module provides deterministic public key generation.
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::wallet::P2PKSigningKey;
use cdk_common::{PublicKey, SECP256K1};

use crate::error::Error;

const CASHU_PURPOSE: u32 = 129372;
const P2PK_PURPOSE: u32 = 10;

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
        ChildNumber::from_hardened_idx(CASHU_PURPOSE)?,
        ChildNumber::from_hardened_idx(P2PK_PURPOSE)?,
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

        let stored_keys = localstore.list_p2pk_keys().await.unwrap();
        assert_eq!(
            pubkey.to_hex(),
            "03381fbf0996b81d49c35bae17a70d71db9a9e802b1af5c2516fc90381f4741e06".to_string()
        );
        assert_eq!(
            pubkey_1.to_hex(),
            "039bbb7a9cd234da13a113cdd8e037a25c66bbf3a77139d652786a1d7e9d73e600".to_string()
        );
        assert_eq!(
            pubkey_2.to_hex(),
            "02ffd52ed54761750d75b67342544cc8da8a0994f84c46d546e0ab574dd3651a29".to_string()
        );
        assert_eq!(
            pubkey_3.to_hex(),
            "02751ab780960ff177c2300e440fddc0850238a78782a1cab7b0ae03c41978d92d".to_string()
        );
        assert_eq!(
            pubkey_4.to_hex(),
            "0391a9ba1c3caf39ca0536d44419a6ceeda922ee61aa651a72a60171499c02b423".to_string()
        );
        assert_eq!(stored_keys.len(), 5);
        assert_eq!(
            stored_keys[0].derivation_path.to_string(),
            "129372'/10'/0'/0'/0"
        );
        assert_eq!(
            stored_keys[1].derivation_path.to_string(),
            "129372'/10'/0'/0'/1"
        );
        assert_eq!(
            stored_keys[2].derivation_path.to_string(),
            "129372'/10'/0'/0'/2"
        );
        assert_eq!(
            stored_keys[3].derivation_path.to_string(),
            "129372'/10'/0'/0'/3"
        );
        assert_eq!(
            stored_keys[4].derivation_path.to_string(),
            "129372'/10'/0'/0'/4"
        );
    }
}
