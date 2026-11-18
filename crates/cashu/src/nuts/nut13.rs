//! NUT-13: Deterministic Secrets
//!
//! <https://github.com/cashubtc/nuts/blob/main/13.md>

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::hashes::{hmac, sha256, Hash, HashEngine, HmacEngine};
use bitcoin::{secp256k1, Network};
use thiserror::Error;
use tracing::instrument;

use super::nut00::{BlindedMessage, PreMint, PreMintSecrets};
use super::nut01::{BlindingFactor, PublicKey, SecretKey};
use super::nut02::{Id, KeySetVersion};
use crate::amount::{FeeAndAmounts, SplitTarget};
use crate::dhke::blind_secret;
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
    #[error("HMAC error: {0}")]
    Hmac(bitcoin::secp256k1::hashes::FromSliceError),
    /// SecretKey Error
    #[error(transparent)]
    SecpError(#[from] bitcoin::secp256k1::Error),
    /// V3 derivation exhausted the attempt space
    #[error("V3 derivation exhausted the attempt space")]
    V3DerivationExhausted,
    /// V3 derivation scope is not exactly 33 bytes
    #[error("V3 derivation scope must be 33 bytes, got {0}")]
    InvalidV3Scope(usize),
}

/// Domain prefix of every HMAC-SHA256 KDF message (V2 and V3)
const KDF_DOMAIN: &[u8] = b"Cashu_KDF_HMAC_SHA256";

/// Derivation types of the V3 KDF (NUT-13)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum V3DerivationType {
    /// Internal private key `k`; the secret is `k*G`. Keyset scope.
    SecretKey = 0x00,
    /// Blinding factor `r`, sampled against the BLS12-381 scalar order. Keyset scope.
    BlindingFactor = 0x01,
    /// NUMS offset `u` of a script-only secret. Keyset scope.
    NumsOffset = 0x02,
    /// A wallet's own leaf key at an index. Keyset scope, suffix `u32_BE(i)`.
    LeafKey = 0x03,
    /// Mint quote lock key. Mint identity scope, per-mint quote counter.
    QuoteLockKey = 0x04,
}

/// Build the V3 KDF message for one attempt
///
/// `DOMAIN || u32_BE(len(scope)) || scope || u64_BE(counter) || type || u32_BE(attempt) || suffix`
fn v3_message(
    scope: &[u8; 33],
    counter: u64,
    derivation_type: V3DerivationType,
    attempt: u32,
    suffix: &[u8],
) -> Vec<u8> {
    let mut message = Vec::with_capacity(KDF_DOMAIN.len() + 4 + 33 + 8 + 1 + 4 + suffix.len());
    message.extend_from_slice(KDF_DOMAIN);
    message.extend_from_slice(&(scope.len() as u32).to_be_bytes());
    message.extend_from_slice(scope);
    message.extend_from_slice(&counter.to_be_bytes());
    message.push(derivation_type as u8);
    message.extend_from_slice(&attempt.to_be_bytes());
    message.extend_from_slice(suffix);
    message
}

fn hmac_sha256(seed: &[u8], message: &[u8]) -> [u8; 32] {
    let mut engine = HmacEngine::<sha256::Hash>::new(seed);
    engine.input(message);
    hmac::Hmac::<sha256::Hash>::from_engine(engine).to_byte_array()
}

/// Rejection-sample a V3 derivation: the first attempt whose digest `accept`s
fn v3_sample<T, F>(
    seed: &[u8],
    scope: &[u8; 33],
    counter: u64,
    derivation_type: V3DerivationType,
    suffix: &[u8],
    accept: F,
) -> Result<(T, u32), Error>
where
    F: Fn(&[u8; 32]) -> Option<T>,
{
    for attempt in 0..=u32::MAX {
        let digest = hmac_sha256(
            seed,
            &v3_message(scope, counter, derivation_type, attempt, suffix),
        );
        if let Some(value) = accept(&digest) {
            return Ok((value, attempt));
        }
    }
    Err(Error::V3DerivationExhausted)
}

/// Sample a secp256k1 private key in `[1, n-1]`
fn v3_sample_secp(
    seed: &[u8],
    scope: &[u8; 33],
    counter: u64,
    derivation_type: V3DerivationType,
    suffix: &[u8],
) -> Result<(SecretKey, u32), Error> {
    v3_sample(seed, scope, counter, derivation_type, suffix, |digest| {
        SecretKey::from_slice(digest).ok()
    })
}

/// The 33-byte scope of a keyset: its full id. A short id is refused.
fn v3_keyset_scope(keyset_id: Id) -> Result<[u8; 33], Error> {
    let bytes = keyset_id.to_bytes();
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::InvalidV3Scope(bytes.len()))
}

/// Derive the V3 internal private key `k` (type `0x00`) for a proof counter
pub fn derive_v3_secret_key(seed: &[u8], keyset_id: Id, counter: u32) -> Result<SecretKey, Error> {
    derive_v3_secret_key_with_attempt(seed, keyset_id, counter).map(|(key, _)| key)
}

fn derive_v3_secret_key_with_attempt(
    seed: &[u8],
    keyset_id: Id,
    counter: u32,
) -> Result<(SecretKey, u32), Error> {
    v3_sample_secp(
        seed,
        &v3_keyset_scope(keyset_id)?,
        u64::from(counter),
        V3DerivationType::SecretKey,
        &[],
    )
}

/// Derive the V3 NUMS offset `u` (type `0x02`) for a proof counter
pub fn derive_v3_nums_offset(seed: &[u8], keyset_id: Id, counter: u32) -> Result<SecretKey, Error> {
    v3_sample_secp(
        seed,
        &v3_keyset_scope(keyset_id)?,
        u64::from(counter),
        V3DerivationType::NumsOffset,
        &[],
    )
    .map(|(key, _)| key)
}

/// Derive a wallet's own V3 leaf key at `index` (type `0x03`) for a proof counter
///
/// `index` is not a leaf's position in a tree: a receiver recovers it by
/// deriving candidates and matching the tree's keys by value.
pub fn derive_v3_leaf_key(
    seed: &[u8],
    keyset_id: Id,
    counter: u32,
    index: u32,
) -> Result<SecretKey, Error> {
    v3_sample_secp(
        seed,
        &v3_keyset_scope(keyset_id)?,
        u64::from(counter),
        V3DerivationType::LeafKey,
        &index.to_be_bytes(),
    )
    .map(|(key, _)| key)
}

/// Derive a V3 mint quote lock key (type `0x04`)
///
/// The scope is the mint's NUT-06 identity public key and `counter` is the
/// wallet's quote counter for that mint, separate from any proof counter.
pub fn derive_v3_quote_lock_key(
    seed: &[u8],
    mint_pubkey: &PublicKey,
    counter: u32,
) -> Result<SecretKey, Error> {
    v3_sample_secp(
        seed,
        &mint_pubkey.to_bytes(),
        u64::from(counter),
        V3DerivationType::QuoteLockKey,
        &[],
    )
    .map(|(key, _)| key)
}

impl Secret {
    /// Create new [`Secret`] from seed
    pub fn from_seed(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        match keyset_id.get_version() {
            KeySetVersion::Version00 => Self::legacy_derive(seed, keyset_id, counter),
            KeySetVersion::Version01 => Self::derive(seed, keyset_id, counter),
            KeySetVersion::Version02 => Ok(Self::from_point(
                &derive_v3_secret_key(seed, keyset_id, counter)?.public_key(),
            )),
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

    fn derive(seed: &[u8], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        let mut message = Vec::new();
        message.extend_from_slice(KDF_DOMAIN);
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

impl BlindingFactor {
    /// Create a new deterministic blinding factor from a seed.
    pub fn from_seed(seed: &[u8; 64], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        match keyset_id.get_version() {
            KeySetVersion::Version00 => Self::legacy_derive(seed, keyset_id, counter),
            KeySetVersion::Version01 => Self::derive(seed, keyset_id, counter),
            KeySetVersion::Version02 => Self::derive_bls(seed, keyset_id, counter),
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

    fn derive(seed: &[u8], keyset_id: Id, counter: u32) -> Result<Self, Error> {
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

    fn derive_bls(seed: &[u8], keyset_id: Id, counter: u32) -> Result<Self, Error> {
        Self::derive_bls_with_attempt(seed, keyset_id, counter).map(|(secret_key, _)| secret_key)
    }

    fn derive_bls_with_attempt(
        seed: &[u8],
        keyset_id: Id,
        counter: u32,
    ) -> Result<(Self, u32), Error> {
        v3_sample(
            seed,
            &v3_keyset_scope(keyset_id)?,
            u64::from(counter),
            V3DerivationType::BlindingFactor,
            &[],
            |digest| Self::bls_from_slice(digest).ok(),
        )
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

        for (counter, amount) in
            (counter..).zip(amount.split_targeted(amount_split_target, fee_and_amounts)?)
        {
            let secret = Secret::from_seed(seed, keyset_id, counter)?;
            let blinding_factor = BlindingFactor::from_seed(seed, keyset_id, counter)?;

            let (blinded, r) =
                blind_secret(&secret, Some(blinding_factor), keyset_id.get_version())?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            let pre_mint = PreMint {
                blinded_message,
                secret: secret.clone(),
                r,
                amount,
                derivation_index: Some(counter),
            };

            pre_mint_secrets.secrets.push(pre_mint);
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

        for counter in counter..(counter + count as u32) {
            let secret = Secret::from_seed(seed, keyset_id, counter)?;
            let blinding_factor = BlindingFactor::from_seed(seed, keyset_id, counter)?;

            let (blinded, r) =
                blind_secret(&secret, Some(blinding_factor), keyset_id.get_version())?;

            let amount = Amount::ZERO;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            let pre_mint = PreMint {
                blinded_message,
                secret: secret.clone(),
                r,
                amount,
                derivation_index: Some(counter),
            };

            pre_mint_secrets.secrets.push(pre_mint);
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

        for i in start_count..end_count {
            let secret = Secret::from_seed(seed, keyset_id, i)?;
            let blinding_factor = BlindingFactor::from_seed(seed, keyset_id, i)?;

            let (blinded, r) =
                blind_secret(&secret, Some(blinding_factor), keyset_id.get_version())?;

            let blinded_message = BlindedMessage::new(Amount::ZERO, keyset_id, blinded);

            let pre_mint = PreMint {
                blinded_message,
                secret: secret.clone(),
                r,
                amount: Amount::ZERO,
                derivation_index: Some(i),
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
            let r = BlindingFactor::from_seed(&seed, keyset_id, i.try_into().unwrap()).unwrap();
            assert_eq!(r, BlindingFactor::from_hex(test_r).unwrap())
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
    fn test_from_seed_blank_zero_amount_returns_empty_batch() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let pre_mint = PreMintSecrets::from_seed_blank(keyset_id, 5, &seed, Amount::ZERO).unwrap();

        assert!(pre_mint.is_empty());
    }

    #[test]
    fn test_from_seed_blank_uses_expected_counter_range() {
        let seed =
            "half depart obvious quality work element tank gorilla view sugar picture humble";
        let mnemonic = Mnemonic::from_str(seed).unwrap();
        let seed: [u8; 64] = mnemonic.to_seed("");
        let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

        let pre_mint =
            PreMintSecrets::from_seed_blank(keyset_id, 5, &seed, Amount::from(3)).unwrap();
        let restored = PreMintSecrets::restore_batch(keyset_id, &seed, 5, 7).unwrap();

        assert_eq!(pre_mint.len(), 2);
        assert_eq!(pre_mint.secrets, restored.secrets);
        assert!(pre_mint
            .secrets
            .iter()
            .all(|secret| secret.amount == Amount::ZERO));
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
            let secret_key = BlindingFactor::from_seed(&seed, keyset_id, i as u32).unwrap();

            // Verify the secret key is valid (32 bytes)
            let secret_bytes = secret_key.secret_bytes();
            assert_eq!(secret_bytes.len(), 32);

            // Test deterministic derivation
            let secret_key2 = BlindingFactor::from_str(test_secret).unwrap();
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

            let secret_key_1 = BlindingFactor::from_seed(&seed, keyset_id_1, counter).unwrap();
            let secret_key_2 = BlindingFactor::from_seed(&seed, keyset_id_2, counter).unwrap();
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
            let secret_key = BlindingFactor::from_seed(&seed, keyset_id, counter).unwrap();

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
        let secret_key = BlindingFactor::from_seed(&seed, keyset_id, counter).unwrap();

        // Verify outputs are valid hex strings of correct length
        assert_eq!(secret.to_string().len(), 64); // 32 bytes as hex
        assert_eq!(secret_key.secret_bytes().len(), 32);
    }

    /// NUT-13 V3 vectors (tests/13-tests.md, cashubtc/nuts#443)
    mod v3_vectors {
        use super::*;

        const SEED_HEX: &str = "6e757431332076332074657374207365656400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        const KEYSET_ID: &str =
            "022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701";

        fn seed() -> [u8; 64] {
            hex::decode(SEED_HEX).unwrap().try_into().unwrap()
        }

        fn keyset_id() -> Id {
            Id::from_str(KEYSET_ID).unwrap()
        }

        #[test]
        fn counters_0_to_3() {
            // (secret_key, secret, blinding_factor, nums_offset, r attempt)
            let vectors = [
                (
                    "930c88ad9dc8531c147862d4ef9c0981200ff1f510c0a9e1c024db725a1d56c6",
                    "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9",
                    "4381a54d5fb006bf9cd63a30e857f0ef4b58a5a123c7cd8909099abe77d5736e",
                    "c4b697b346799e55a80ba7a0b6f7670a7f62a985c97de685dc693e87dc6d137a",
                    0,
                ),
                (
                    "a2ac179175fc3bb7c8cf05542193017a0d33b3316ea5d8a97777c7660d2a55b6",
                    "0385fe6eb0608c6b5241e0fd86ebbd5dd30a6a97a0014a110c0c6297001147e5be",
                    "2349fa461401f98e1ffa80d34baf1c11bbe62014bbd076985229f0706cda83f1",
                    "1b864ba8bba906c1bd1b56e58c1b0fe4ee8d3bea42849309102c81a8854f312d",
                    3,
                ),
                (
                    "fec1f9c77a85ea144b6ea2ecd90a61a226855b4ed692a7e8c7db7afb600f7205",
                    "0390617487ad222beef9f247dbfbd5993313a2d5235add388bd868969c594794a2",
                    "7121cd4ce638011afaea356eefe1834b03a86080064c7c28f1eab45c32d7b06e",
                    "b19ab48593e7ad6fc3c68c23697b6b2544920a1949ca234a6d10a2cdb6455922",
                    0,
                ),
                (
                    "1aa7018e67ac4655d3d6dd120a2c715adb6bcd6b54dbf90a685ae452916edb61",
                    "039e49f114e0ab5f616b996e93f04ae265e29282df093c08996c093f1fcbb4385f",
                    "46186ec0f98f00aaf51a7cb59472c65e35a33f94e2785b6b384d1666b9c1759c",
                    "3ee8a5612ad8ff4a6f5b7bcf5b6bef7c7a2379fc789401e8e03afbf32da6dfad",
                    1,
                ),
            ];
            let seed = seed();
            for (counter, (k, secret, r, u, r_attempt)) in vectors.into_iter().enumerate() {
                let counter = counter as u32;
                let (secret_key, k_attempt) =
                    derive_v3_secret_key_with_attempt(&seed, keyset_id(), counter).unwrap();
                assert_eq!(secret_key.to_secret_hex(), k, "k at counter {counter}");
                assert_eq!(k_attempt, 0, "k attempt at counter {counter}");
                assert_eq!(
                    Secret::from_seed(&seed, keyset_id(), counter)
                        .unwrap()
                        .to_string(),
                    secret
                );
                let (blinding_factor, attempt) =
                    BlindingFactor::derive_bls_with_attempt(&seed, keyset_id(), counter).unwrap();
                assert_eq!(blinding_factor.to_secret_hex(), r, "r at counter {counter}");
                assert_eq!(attempt, r_attempt, "r attempt at counter {counter}");
                assert_eq!(
                    BlindingFactor::from_seed(&seed, keyset_id(), counter)
                        .unwrap()
                        .to_secret_hex(),
                    r
                );
                assert_eq!(
                    derive_v3_nums_offset(&seed, keyset_id(), counter)
                        .unwrap()
                        .to_secret_hex(),
                    u
                );
            }
        }

        #[test]
        fn y_hashes_decoded_secret_bytes() {
            let secret = Secret::from_seed(&seed(), keyset_id(), 0).unwrap();
            let y = crate::dhke::hash_secret_to_curve(&secret, KeySetVersion::Version02).unwrap();
            assert_eq!(
                y.to_hex(),
                "90f53b3088936b80ecfcc065a0746c5c1843ffd4527dbe3b9aba5a6109aed338ddc0f109511e9dfd37342aa99dd816d8"
            );
        }

        #[test]
        fn leaf_keys() {
            let vectors = [
                (
                    "e6573de10fb740e60350346f7a90e15dfff01efb476c745018f59b6ab5132644",
                    "036ed2c40140c79e5080a15e88cb55f7c08c0107b465cfef8ddcfe66a2ba3532f9",
                ),
                (
                    "2ab582bed4c0a4c0d746b8bfbe906cf7d7becf88f4dfe66cd194851c64d44bb1",
                    "0369e4632867bd810a4a7065494b48c7fda2dd6ec4eba4fd93ce545bd32d5c45df",
                ),
                (
                    "520f39607492fad03cf0b2c44929680ef3b0955adf20a9b2d9b64b18bffaeaa4",
                    "037864253663cfeadc8bd3d5d573ca1858378951bbd7fe971712940f38c4c47c86",
                ),
            ];
            for (index, (privkey, pubkey)) in vectors.into_iter().enumerate() {
                let key = derive_v3_leaf_key(&seed(), keyset_id(), 0, index as u32).unwrap();
                assert_eq!(key.to_secret_hex(), privkey);
                assert_eq!(key.public_key().to_hex(), pubkey);
            }
        }

        #[test]
        fn quote_lock_keys() {
            let mint_pubkey = PublicKey::from_hex(
                "0338596797cef0627f653cd6568387361b00314add55d9f1ea9c94f46ae421e3da",
            )
            .unwrap();
            let vectors = [
                (
                    "8140af6b627ddf38f9148c7266dca12d396de9266859648df699727c5b205a99",
                    "0292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d209812",
                ),
                (
                    "5980da641d9391abed79f528dbea0b85a61cff419bc93bce744d105779413b7a",
                    "02b357c1ec7bdd73e4ede25d68619ee607c632409e89bcc518a82370b3f499615d",
                ),
            ];
            for (counter, (privkey, pubkey)) in vectors.into_iter().enumerate() {
                let key = derive_v3_quote_lock_key(&seed(), &mint_pubkey, counter as u32).unwrap();
                assert_eq!(key.to_secret_hex(), privkey);
                assert_eq!(key.public_key().to_hex(), pubkey);
            }
        }

        #[test]
        fn message_layout() {
            let scope = [0xaa; 33];
            let message = v3_message(
                &scope,
                0x0102_0304_0506_0708,
                V3DerivationType::LeafKey,
                7,
                &9u32.to_be_bytes(),
            );
            let mut expected = b"Cashu_KDF_HMAC_SHA256".to_vec();
            expected.extend_from_slice(&[0, 0, 0, 33]);
            expected.extend_from_slice(&scope);
            expected.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            expected.push(0x03);
            expected.extend_from_slice(&[0, 0, 0, 7]);
            expected.extend_from_slice(&[0, 0, 0, 9]);
            assert_eq!(message, expected);
        }

        #[test]
        fn short_keyset_scope_is_refused() {
            // A v1 id is 33 bytes too, so build a scope from a v0 (8-byte) id.
            let short = Id::from_str("009a1f293253e41e").unwrap();
            assert!(matches!(
                derive_v3_secret_key(&seed(), short, 0),
                Err(Error::InvalidV3Scope(8))
            ));
        }
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
            (end_count - start_count) as usize
        );

        // Verify each secret in the batch
        for (i, pre_mint) in pre_mint_secrets.secrets.iter().enumerate() {
            let counter = start_count + i as u32;
            let expected_secret = Secret::from_seed(&seed, keyset_id, counter).unwrap();
            assert_eq!(pre_mint.secret, expected_secret);
        }
    }
}
