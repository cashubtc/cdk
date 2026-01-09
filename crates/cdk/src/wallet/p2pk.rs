use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::database::{self, WalletDatabase};
use cdk_common::wallet::P2PKSigningKey;
use cdk_common::{PublicKey, SECP256K1};

use crate::error::Error;

/// Generates and stores public key in database
pub async fn generate_public_key(
    localstore: &Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    seed: &[u8; 64],
) -> Result<PublicKey, Error> {
    let public_keys = localstore.list_p2pk_keys().await?;

    let mut last_derivation_index = 0;

    for public_key in public_keys {
        if public_key.derivation_index > last_derivation_index {
            last_derivation_index = public_key.derivation_index;
        }
    }
    last_derivation_index += 1;

    let derivation_path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_hardened_idx(last_derivation_index)?,
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
