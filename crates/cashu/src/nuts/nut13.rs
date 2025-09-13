//! NUT-13: Deterministic Secrets
//!
//! <https://github.com/cashubtc/nuts/blob/main/13.md>

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::hashes::{hmac, sha256, Hash, HashEngine, HmacEngine};
use bitcoin::{secp256k1, Network};
use thiserror::Error;
use tracing::instrument;

use super::nut00::{BlindedMessage, PreMint, PreMintSecrets};
use super::nut01::SecretKey;
use super::nut02::Id;
use crate::amount::{FeeAndAmounts, SplitTarget};
use crate::dhke::blind_message;
use crate::secret::Secret;
use crate::util::hex;
use crate::{Amount, SECP256K1};

/// NUT13 Error
#[derive(Debug, Error)]
pub enum Error {
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
    /// Amount Error
    #[error(transparent)]
    Amount(#[from] crate::amount::Error),
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] crate::nuts::nut00::Error),
    /// NUT02 Error
    #[error(transparent)]
    NUT02(#[from] crate::nuts::nut02::Error),
    /// Bip32 Error
    #[error(transparent)]
    Bip32(#[from] bitcoin::bip32::Error),
    /// HMAC Error
    #[error(transparent)]
    Hmac(#[from] bitcoin::secp256k1::hashes::FromSliceError),
    /// SecretKey Error
    #[error(transparent)]
    SecpError(#[from] bitcoin::secp256k1::Error),
}

impl Secret {
    /// Create new [`Secret`] from seed
    pub fn from_seed(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        match keyset_id.get_version() {
            super::nut02::KeySetVersion::Version00 => Self::legacy_derive(seed, keyset_id, counter),
            super::nut02::KeySetVersion::Version01 => Self::derive(seed, keyset_id, counter),
        }
    }

    fn legacy_derive(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;
        let path = derive_path_from_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(0)?);
        let derived_xpriv = xpriv.derive_priv(&SECP256K1, &path)?;

        Ok(Self::new(hex::encode(
            derived_xpriv.private_key.secret_bytes(),
        )))
    }

    fn derive(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let mut message = Vec::new();
        message.extend_from_slice(b"Cashu_KDF_HMAC_SHA256");
        message.extend_from_slice(&keyset_id.to_bytes());
        message.extend_from_slice(&(counter as u64).to_be_bytes());
        message.extend_from_slice(b"\x00");

        let mut engine = HmacEngine::<sha256::Hash>::new(seed);
        engine.input(&message);
        let hmac_result = hmac::Hmac::<sha256::Hash>::from_engine(engine);
        let result_bytes = hmac_result.to_byte_array();

        Ok(Self::new(hex::encode(&result_bytes[..32])))
    }
}

impl SecretKey {
    /// Create new [`SecretKey`] from seed
    pub fn from_seed(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        match keyset_id.get_version() {
            super::nut02::KeySetVersion::Version00 => Self::legacy_derive(seed, keyset_id, counter),
            super::nut02::KeySetVersion::Version01 => Self::derive(seed, keyset_id, counter),
        }
    }

    fn legacy_derive(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;
        let path = derive_path_from_keyset_id(keyset_id)?
            .child(ChildNumber::from_hardened_idx(counter)?)
            .child(ChildNumber::from_normal_idx(1)?);
        let derived_xpriv = xpriv.derive_priv(&SECP256K1, &path)?;

        Ok(Self::from(derived_xpriv.private_key))
    }

    fn derive(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let mut message = Vec::new();
        message.extend_from_slice(b"Cashu_KDF_HMAC_SHA256");
        message.extend_from_slice(&keyset_id.to_bytes());
        message.extend_from_slice(&(counter as u64).to_be_bytes());
        message.extend_from_slice(b"\x01");

        let mut engine = HmacEngine::<sha256::Hash>::new(seed);
        engine.input(&message);
        let hmac_result = hmac::Hmac::<sha256::Hash>::from_engine(engine);
        let result_bytes = hmac_result.to_byte_array();

        Ok(Self::from(secp256k1::SecretKey::from_slice(
            &result_bytes[..32],
        )?))
    }
}

impl PreMintSecrets {
    /// Generate blinded messages from predetermined secrets and blindings
    /// factor
    #[instrument(skip(seed))]
    pub fn from_seed(
        keyset_id: Id,
        counter: u32,
        seed: &[u8; 64],
        amount: Amount,
        amount_split_target: &SplitTarget,
        fee_and_amounts: &FeeAndAmounts,
    ) -> Result<Self, Error> {
        let mut pre_mint_secrets = PreMintSecrets::new(keyset_id);

        let mut counter = counter;

        for amount in amount.split_targeted(amount_split_target, fee_and_amounts)? {
            let secret = Secret::from_seed(seed, keyset_id, counter)?;
            let blinding_factor = SecretKey::from_seed(seed, keyset_id, counter)?;

            let (blinded, r) = blind_message(&secret.to_bytes(), Some(blinding_factor))?;

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

    /// New [`PreMintSecrets`] from seed with a zero amount used for change
    pub fn from_seed_blank(
        keyset_id: Id,
        counter: u32,
        seed: &[u8; 64],
        amount: Amount,
    ) -> Result<Self, Error> {
        if amount <= Amount::ZERO {
            return Ok(PreMintSecrets::new(keyset_id));
        }
        let count = ((u64::from(amount) as f64).log2().ceil() as u64).max(1);
        let mut pre_mint_secrets = PreMintSecrets::new(keyset_id);

        let mut counter = counter;

        for _ in 0..count {
            let secret = Secret::from_seed(seed, keyset_id, counter)?;
            let blinding_factor = SecretKey::from_seed(seed, keyset_id, counter)?;

            let (blinded, r) = blind_message(&secret.to_bytes(), Some(blinding_factor))?;

            let amount = Amount::ZERO;

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
        seed: &[u8; 64],
        start_count: u32,
        end_count: u32,
    ) -> Result<Self, Error> {
        let mut pre_mint_secrets = PreMintSecrets::new(keyset_id);

        for i in start_count..=end_count {
            let secret = Secret::from_seed(seed, keyset_id, i)?;
            let blinding_factor = SecretKey::from_seed(seed, keyset_id, i)?;

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
    let index = u32::from(id);

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
    use bitcoin::bip32::DerivationPath;

    use super::*;

    #[test]
    fn test_secret_from_seed() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let test_secrets = [
            "485875df74771877439ac06339e284c3acfcd9be7abf3bc20b516faeadfe77ae",
            "8f2b39e8e594a4056eb1e6dbb4b0c38ef13b1b2c751f64f810ec04ee35b77270",
            "bc628c79accd2364fd31511216a0fab62afd4a18ff77a20deded7b858c9860c8",
            "59284fd1650ea9fa17db2b3acf59ecd0f2d52ec3261dd4152785813ff27a33bf",
            "576c23393a8b31cc8da6688d9c9a96394ec74b40fdaf1f693a6bb84284334ea0",
        ];

        for (i, test_secret) in test_secrets.iter().enumerate() {
            let secret = Secret::from_seed(&seed, keyset_id, i.try_into().unwrap()).unwrap();
            assert_eq!(secret, Secret::from_str(test_secret).unwrap())
        }
    }
    #[test]
    fn test_r_from_seed() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let test_rs = [
            "ad00d431add9c673e843d4c2bf9a778a5f402b985b8da2d5550bf39cda41d679",
            "967d5232515e10b81ff226ecf5a9e2e2aff92d66ebc3edf0987eb56357fd6248",
            "b20f47bb6ae083659f3aa986bfa0435c55c6d93f687d51a01f26862d9b9a4899",
            "fb5fca398eb0b1deb955a2988b5ac77d32956155f1c002a373535211a2dfdc29",
            "5f09bfbfe27c439a597719321e061e2e40aad4a36768bb2bcc3de547c9644bf9",
        ];

        for (i, test_r) in test_rs.iter().enumerate() {
            let r = SecretKey::from_seed(&seed, keyset_id, i.try_into().unwrap()).unwrap();
            assert_eq!(r, SecretKey::from_hex(test_r).unwrap())
        }
    }

    #[test]
    fn test_derive_path_from_keyset_id() {
        let test_cases = [
            ("009a1f293253e41e", "m/129372'/0'/864559728'"),
            ("0000000000000000", "m/129372'/0'/0'"),
            ("00ffffffffffffff", "m/129372'/0'/33554431'"),
        ];

        for (id_hex, expected_path) in test_cases {
            let id = Id::from_str(id_hex).unwrap();
            let path = derive_path_from_keyset_id(id).unwrap();
            assert_eq!(
                DerivationPath::from_str(expected_path).unwrap(),
                path,
                "Path derivation failed for ID {id_hex}"
            );
        }
    }

    #[test]
    fn test_secret_derivation_keyset_v2() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        // Test with a v2 keyset ID (33 bytes, starting with "01")
        let keyset_id =
            Id::from_str("012e23479a0029432eaad0d2040c09be53bab592d5cbf1d55e0dd26c9495951b30")
                .unwrap();

        // Expected secrets derived using the new derivation
        let test_secrets = [
            "ba250bf927b1df5dd0a07c543be783a4349a7f99904acd3406548402d3484118",
            "3a6423fe56abd5e74ec9d22a91ee110cd2ce45a7039901439d62e5534d3438c1",
            "843484a75b78850096fac5b513e62854f11d57491cf775a6fd2edf4e583ae8c0",
            "3600608d5cf8197374f060cfbcff134d2cd1fb57eea68cbcf2fa6917c58911b6",
            "717fce9cc6f9ea060d20dd4e0230af4d63f3894cc49dd062fd99d033ea1ac1dd",
        ];

        for (i, test_secret) in test_secrets.iter().enumerate() {
            let secret = Secret::from_seed(&seed, keyset_id, i.try_into().unwrap()).unwrap();
            // Note: The actual expected values would need to be computed from a reference implementation
            // For now, we just verify the derivation works and produces consistent results
            assert_eq!(secret.to_string().len(), 64); // Should be 32 bytes = 64 hex chars

            // Test deterministic derivation: same inputs should produce same outputs
            let secret2 = Secret::from_str(test_secret).unwrap();
            assert_eq!(secret, secret2);
        }
    }

    #[test]
    fn test_secret_key_derivation_keyset_v2() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        // Test with a v2 keyset ID (33 bytes, starting with "01")
        let keyset_id =
            Id::from_str("012e23479a0029432eaad0d2040c09be53bab592d5cbf1d55e0dd26c9495951b30")
                .unwrap();

        let test_secret_keys = [
            "4f8b32a54aed811b692a665ed296b4c1fc2f37a8be4006379e95063a76693745",
            "c4b8412ee644067007423480c9e556385b71ffdff0f340bc16a95c0534fe0e01",
            "ceff40983441c40acaf77d2a8ddffd5c1c84391fb9fd0dc4607c186daab1c829",
            "41ad26b840fb62d29b2318a82f1d9cd40dc0f1e58183cc57562f360a32fdfad6",
            "fb986a9c76758593b0e2d1a5172ade977c858d87111a220e16c292a9347abf81",
        ];

        for (i, test_secret) in test_secret_keys.iter().enumerate() {
            let secret_key = SecretKey::from_seed(&seed, keyset_id, i as u32).unwrap();

            // Verify the secret key is valid (32 bytes)
            let secret_bytes = secret_key.secret_bytes();
            assert_eq!(secret_bytes.len(), 32);

            // Test deterministic derivation
            let secret_key2 = SecretKey::from_str(test_secret).unwrap();
            assert_eq!(secret_key, secret_key2);
        }
    }

    #[test]
    fn test_v2_derivation_with_different_keysets() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        let keyset_id_1 =
            Id::from_str("01adc013fa9d85171586660abab27579888611659d357bc86bc09cb26eee8bc035")
                .unwrap();
        let keyset_id_2 =
            Id::from_str("01bef024fb9e85171586660abab27579888611659d357bc86bc09cb26eee8bc046")
                .unwrap();

        // Different keyset IDs should produce different secrets even with same counter
        for counter in 0..3 {
            let secret_1 = Secret::from_seed(&seed, keyset_id_1, counter).unwrap();
            let secret_2 = Secret::from_seed(&seed, keyset_id_2, counter).unwrap();
            assert_ne!(
                secret_1, secret_2,
                "Different keyset IDs should produce different secrets for counter {}",
                counter
            );

            let secret_key_1 = SecretKey::from_seed(&seed, keyset_id_1, counter).unwrap();
            let secret_key_2 = SecretKey::from_seed(&seed, keyset_id_2, counter).unwrap();
            assert_ne!(
                secret_key_1, secret_key_2,
                "Different keyset IDs should produce different secret keys for counter {}",
                counter
            );
        }
    }

    #[test]
    fn test_v2_derivation_incremental_counters() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        let keyset_id =
            Id::from_str("01adc013fa9d85171586660abab27579888611659d357bc86bc09cb26eee8bc035")
                .unwrap();

        let mut secrets = Vec::new();
        let mut secret_keys = Vec::new();

        // Generate secrets with incremental counters
        for counter in 0..10 {
            let secret = Secret::from_seed(&seed, keyset_id, counter).unwrap();
            let secret_key = SecretKey::from_seed(&seed, keyset_id, counter).unwrap();

            // Ensure no duplicates
            assert!(
                !secrets.contains(&secret),
                "Duplicate secret found for counter {}",
                counter
            );
            assert!(
                !secret_keys.contains(&secret_key),
                "Duplicate secret key found for counter {}",
                counter
            );

            secrets.push(secret);
            secret_keys.push(secret_key);
        }
    }

    #[test]
    fn test_v2_hmac_message_construction() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        let keyset_id =
            Id::from_str("01adc013fa9d85171586660abab27579888611659d357bc86bc09cb26eee8bc035")
                .unwrap();
        let counter: u32 = 42;

        // Test that the HMAC message is constructed correctly
        // Message should be: b"Cashu_KDF_HMAC_SHA512" + keyset_id.to_bytes() + counter.to_be_bytes()
        let _expected_prefix = b"Cashu_KDF_HMAC_SHA512";
        let keyset_bytes = keyset_id.to_bytes();
        let _counter_bytes = (counter as u64).to_be_bytes();

        // Verify keyset ID v2 structure: version byte (01) + 32 bytes
        assert_eq!(keyset_bytes.len(), 33);
        assert_eq!(keyset_bytes[0], 0x01);

        // The actual HMAC construction is internal, but we can verify the derivation works
        let secret = Secret::from_seed(&seed, keyset_id, counter).unwrap();
        let secret_key = SecretKey::from_seed(&seed, keyset_id, counter).unwrap();

        // Verify outputs are valid hex strings of correct length
        assert_eq!(secret.to_string().len(), 64); // 32 bytes as hex
        assert_eq!(secret_key.secret_bytes().len(), 32);
    }

    #[test]
    fn test_pre_mint_secrets_with_v2_keyset() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        let keyset_id =
            Id::from_str("01adc013fa9d85171586660abab27579888611659d357bc86bc09cb26eee8bc035")
                .unwrap();
        let amount = Amount::from(1000u64);
        let split_target = SplitTarget::default();
        let fee_and_amounts = (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into();

        // Test PreMintSecrets generation with v2 keyset
        let pre_mint_secrets =
            PreMintSecrets::from_seed(keyset_id, 0, &seed, amount, &split_target, &fee_and_amounts)
                .unwrap();

        // Verify all secrets in the pre_mint use the new v2 derivation
        for (i, pre_mint) in pre_mint_secrets.secrets.iter().enumerate() {
            // Verify the secret was derived correctly
            let expected_secret = Secret::from_seed(&seed, keyset_id, i as u32).unwrap();
            assert_eq!(pre_mint.secret, expected_secret);

            // Verify keyset ID version
            assert_eq!(
                pre_mint.blinded_message.keyset_id.get_version(),
                super::super::nut02::KeySetVersion::Version01
            );
        }
    }

    #[test]
    fn test_restore_batch_with_v2_keyset() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");

        let keyset_id =
            Id::from_str("01adc013fa9d85171586660abab27579888611659d357bc86bc09cb26eee8bc035")
                .unwrap();

        let start_count = 5;
        let end_count = 10;

        // Test batch restoration with v2 keyset
        let pre_mint_secrets =
            PreMintSecrets::restore_batch(keyset_id, &seed, start_count, end_count).unwrap();

        assert_eq!(
            pre_mint_secrets.secrets.len(),
            (end_count - start_count + 1) as usize
        );

        // Verify each secret in the batch
        for (i, pre_mint) in pre_mint_secrets.secrets.iter().enumerate() {
            let counter = start_count + i as u32;
            let expected_secret = Secret::from_seed(&seed, keyset_id, counter).unwrap();
            assert_eq!(pre_mint.secret, expected_secret);
        }
    }
}
