//! NUT-13: Deterministic Secrets
//!
//! <https://github.com/cashubtc/nuts/blob/main/13.md>

use bitcoin::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey};

use super::nut00::{BlindedMessage, PreMint, PreMintSecrets};
use super::nut01::SecretKey;
use super::nut02::Id;
use crate::amount::SplitTarget;
use crate::dhke::blind_message;
use crate::error::Error;
use crate::secret::Secret;
use crate::util::hex;
use crate::{Amount, SECP256K1};

impl Secret {
    pub fn from_xpriv(xpriv: ExtendedPrivKey, keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let path = derive_path_from_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(0)?);
        let derived_xpriv = xpriv.derive_priv(&SECP256K1, &path)?;

        Ok(Self::new(hex::encode(
            derived_xpriv.private_key.secret_bytes(),
        )))
    }
}

impl SecretKey {
    pub fn from_xpriv(xpriv: ExtendedPrivKey, keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let path = derive_path_from_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(1)?);
        let derived_xpriv = xpriv.derive_priv(&SECP256K1, &path)?;

        Ok(Self::from(derived_xpriv.private_key))
    }
}

impl PreMintSecrets {
    /// Generate blinded messages from predetermined secrets and blindings
    /// factor
    pub fn from_xpriv(
        keyset_id: Id,
        counter: u32,
        xpriv: ExtendedPrivKey,
        amount: Amount,
        zero_amount: bool,
        amount_split_target: &SplitTarget,
    ) -> Result<Self, Error> {
        let mut pre_mint_secrets = PreMintSecrets::default();

        let mut counter = counter;

        for amount in amount.split_targeted(amount_split_target) {
            let secret = Secret::from_xpriv(xpriv, keyset_id, counter)?;
            let blinding_factor = SecretKey::from_xpriv(xpriv, keyset_id, counter)?;

            let (blinded, r) = blind_message(&secret.to_bytes(), Some(blinding_factor))?;

            let amount = if zero_amount { Amount::ZERO } else { amount };

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            let pre_mint = PreMint {
                blinded_message,
                secret: secret.clone(),
                r,
                amount,
            };

            pre_mint_secrets.secrets.push(pre_mint);
            counter += 1;
        }

        Ok(pre_mint_secrets)
    }

    /// Generate blinded messages from predetermined secrets and blindings
    /// factor
    pub fn restore_batch(
        keyset_id: Id,
        xpriv: ExtendedPrivKey,
        start_count: u32,
        end_count: u32,
    ) -> Result<Self, Error> {
        let mut pre_mint_secrets = PreMintSecrets::default();

        for i in start_count..=end_count {
            let secret = Secret::from_xpriv(xpriv, keyset_id, i)?;
            let blinding_factor = SecretKey::from_xpriv(xpriv, keyset_id, i)?;

            let (blinded, r) = blind_message(&secret.to_bytes(), Some(blinding_factor))?;

            let blinded_message = BlindedMessage::new(Amount::ZERO, keyset_id, blinded);

            let pre_mint = PreMint {
                blinded_message,
                secret: secret.clone(),
                r,
                amount: Amount::ZERO,
            };

            pre_mint_secrets.secrets.push(pre_mint);
        }

        Ok(pre_mint_secrets)
    }
}

fn derive_path_from_keyset_id(id: Id) -> Result<DerivationPath, Error> {
    let index = (u64::try_from(id)? % (2u64.pow(31) - 1)) as u32;
    let keyset_child_number = ChildNumber::from_hardened_idx(index)?;
    Ok(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(129372)?,
        ChildNumber::from_hardened_idx(0)?,
        keyset_child_number,
    ]))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bip39::Mnemonic;
    use bitcoin::Network;

    use super::*;

    #[test]
    fn test_secret_from_seed() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let xpriv = ExtendedPrivKey::new_master(Network::Bitcoin, &seed).unwrap();
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let test_secrets = [
            "485875df74771877439ac06339e284c3acfcd9be7abf3bc20b516faeadfe77ae",
            "8f2b39e8e594a4056eb1e6dbb4b0c38ef13b1b2c751f64f810ec04ee35b77270",
            "bc628c79accd2364fd31511216a0fab62afd4a18ff77a20deded7b858c9860c8",
            "59284fd1650ea9fa17db2b3acf59ecd0f2d52ec3261dd4152785813ff27a33bf",
            "576c23393a8b31cc8da6688d9c9a96394ec74b40fdaf1f693a6bb84284334ea0",
        ];

        for (i, test_secret) in test_secrets.iter().enumerate() {
            let secret = Secret::from_xpriv(xpriv, keyset_id, i.try_into().unwrap()).unwrap();
            assert_eq!(secret, Secret::from_str(test_secret).unwrap())
        }
    }
    #[test]
    fn test_r_from_seed() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let xpriv = ExtendedPrivKey::new_master(Network::Bitcoin, &seed).unwrap();
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let test_rs = [
            "ad00d431add9c673e843d4c2bf9a778a5f402b985b8da2d5550bf39cda41d679",
            "967d5232515e10b81ff226ecf5a9e2e2aff92d66ebc3edf0987eb56357fd6248",
            "b20f47bb6ae083659f3aa986bfa0435c55c6d93f687d51a01f26862d9b9a4899",
            "fb5fca398eb0b1deb955a2988b5ac77d32956155f1c002a373535211a2dfdc29",
            "5f09bfbfe27c439a597719321e061e2e40aad4a36768bb2bcc3de547c9644bf9",
        ];

        for (i, test_r) in test_rs.iter().enumerate() {
            let r = SecretKey::from_xpriv(xpriv, keyset_id, i.try_into().unwrap()).unwrap();
            assert_eq!(r, SecretKey::from_hex(test_r).unwrap())
        }
    }
}
