//! This module provides deterministic public key generation.

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::{PublicKey, SECP256K1};

use crate::error::Error;

/// This purpose are being used because in base of this PR: https://github.com/cashubtc/nuts/pull/331
/// It's not the same purpose as the cashu purpose because of production code already being used in
/// the coco wallet
pub const P2PK_PURPOSE: u32 = 129373;

/// account used for P2PK derivation
pub const P2PK_ACCOUNT: u32 = 10;

/// Generates and stores public key in database
pub async fn generate_public_key(
    derivation_path: &DerivationPath,
    seed: &[u8; 64],
) -> Result<PublicKey, Error> {
    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;

    let derived_key = xpriv.derive_priv(&SECP256K1, &derivation_path)?.private_key;
    let pubkey = PublicKey::from(derived_key.public_key(&SECP256K1));

    Ok(pubkey)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bip39::Mnemonic;
    use bitcoin::bip32::{ChildNumber, DerivationPath};

    use super::*;

    #[tokio::test]
    async fn nut13_test_vector() {
        let mnemonic = Mnemonic::from_str(
            "half depart obvious quality work element tank gorilla view sugar picture humble",
        )
        .unwrap();

        let seed = mnemonic.to_seed_normalized("");

        let derivation_path_0 = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(P2PK_PURPOSE).unwrap(),
            ChildNumber::from_hardened_idx(P2PK_ACCOUNT).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(0).unwrap(),
        ]);
        let pubkey = generate_public_key(&derivation_path_0, &seed)
            .await
            .unwrap();

        let derivation_path_1 = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(P2PK_PURPOSE).unwrap(),
            ChildNumber::from_hardened_idx(P2PK_ACCOUNT).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(1).unwrap(),
        ]);
        let pubkey_1 = generate_public_key(&derivation_path_1, &seed)
            .await
            .unwrap();
        let derivation_path_2 = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(P2PK_PURPOSE).unwrap(),
            ChildNumber::from_hardened_idx(P2PK_ACCOUNT).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(2).unwrap(),
        ]);
        let pubkey_2 = generate_public_key(&derivation_path_2, &seed)
            .await
            .unwrap();
        let derivation_path_3 = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(P2PK_PURPOSE).unwrap(),
            ChildNumber::from_hardened_idx(P2PK_ACCOUNT).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(3).unwrap(),
        ]);
        let pubkey_3 = generate_public_key(&derivation_path_3, &seed)
            .await
            .unwrap();
        let derivation_path_4 = DerivationPath::from(vec![
            ChildNumber::from_hardened_idx(P2PK_PURPOSE).unwrap(),
            ChildNumber::from_hardened_idx(P2PK_ACCOUNT).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_hardened_idx(0).unwrap(),
            ChildNumber::from_normal_idx(4).unwrap(),
        ]);
        let pubkey_4 = generate_public_key(&derivation_path_4, &seed)
            .await
            .unwrap();

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

        assert_eq!(derivation_path_4.to_string(), "129373'/10'/0'/0'/4");
        assert_eq!(derivation_path_3.to_string(), "129373'/10'/0'/0'/3");
        assert_eq!(derivation_path_2.to_string(), "129373'/10'/0'/0'/2");
        assert_eq!(derivation_path_1.to_string(), "129373'/10'/0'/0'/1");
        assert_eq!(derivation_path_0.to_string(), "129373'/10'/0'/0'/0");
    }
}
