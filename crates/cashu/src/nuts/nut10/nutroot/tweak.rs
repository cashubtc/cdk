//! The taproot-style tweak
//!
//! `t = tagged_hash("Cashu_NutrootTweak", K33 || merkle_root)`, reduced mod
//! the curve order. The empty tweak omits the root. Unlike BIP341, `K` is a
//! 33-byte compressed key and an out-of-range hash is reduced, not rejected.

use bitcoin::secp256k1::constants::CURVE_ORDER;
use bitcoin::secp256k1::Scalar;

use super::internal_key::add_scalar_g;
use super::Error;
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::util::tagged_hash::tagged_hash_parts;

/// Tag of the tweak hash
pub const TWEAK_TAG: &str = "Cashu_NutrootTweak";

/// A tweak scalar `t`, reduced mod `n`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tweak([u8; 32]);

impl Tweak {
    /// Big-endian bytes of `t`
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    fn scalar(&self) -> Scalar {
        Scalar::from_be_bytes(self.0).expect("tweak is reduced mod n")
    }
}

/// `t` for internal key `K` and an optional merkle root
///
/// `None` gives the empty tweak, `tagged_hash("Cashu_NutrootTweak", K)`.
pub fn tweak(internal_key: &PublicKey, merkle_root: Option<&[u8; 32]>) -> Tweak {
    let key = internal_key.to_bytes();
    let hash = match merkle_root {
        Some(root) => tagged_hash_parts(TWEAK_TAG, &[&key, root]),
        None => tagged_hash_parts(TWEAK_TAG, &[&key]),
    };
    Tweak(reduce_mod_n(hash))
}

/// The secret `P = K + t*G`
pub fn tweaked_key(internal_key: &PublicKey, tweak: &Tweak) -> Result<PublicKey, Error> {
    add_scalar_g(internal_key, &tweak.scalar())
}

/// The key-path private key `p' = (k + t) mod n`
pub fn tweaked_secret_key(secret_key: &SecretKey, tweak: &Tweak) -> Result<SecretKey, Error> {
    secret_key
        .add_tweak(&tweak.scalar())
        .map_err(|_| Error::InvalidTweak)
}

/// Reduce 32 big-endian bytes mod `n`. `2^256 < 2n`, so one subtraction suffices.
fn reduce_mod_n(bytes: [u8; 32]) -> [u8; 32] {
    if bytes < CURVE_ORDER {
        return bytes;
    }
    let mut out = [0u8; 32];
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let mut diff = i16::from(bytes[i]) - i16::from(CURVE_ORDER[i]) - borrow;
        borrow = 0;
        if diff < 0 {
            diff += 256;
            borrow = 1;
        }
        out[i] = diff as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    fn key(n: u8) -> SecretKey {
        SecretKey::from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap()
    }

    /// tests/10-tests.md "Empty tweak"
    #[test]
    fn empty_tweak_vector() {
        let k = key(3);
        let t = tweak(&k.public_key(), None);
        assert_eq!(
            hex::encode(t.to_bytes()),
            "764c0e0da0d17acb5cc863fbe939211869e04e522c017171a0b71e91d5b69908"
        );
        assert_eq!(
            tweaked_key(&k.public_key(), &t).unwrap().to_hex(),
            "03b2bb251c006ae42d9c19f3157d02b3c347b1fb6512885225a8699a06d9233aee"
        );
        let p = tweaked_secret_key(&k, &t).unwrap();
        assert_eq!(
            p.to_secret_hex(),
            "764c0e0da0d17acb5cc863fbe939211869e04e522c017171a0b71e91d5b6990b"
        );
        assert_eq!(p.public_key(), tweaked_key(&k.public_key(), &t).unwrap());
    }

    #[test]
    fn reduction_mod_n() {
        assert_eq!(reduce_mod_n([0u8; 32]), [0u8; 32]);
        assert_eq!(reduce_mod_n(CURVE_ORDER), [0u8; 32]);
        let mut n_plus_5 = CURVE_ORDER;
        n_plus_5[31] += 5;
        let mut five = [0u8; 32];
        five[31] = 5;
        assert_eq!(reduce_mod_n(n_plus_5), five);
        // 2^256 - 1 - n
        let reduced = reduce_mod_n([0xff; 32]);
        assert_eq!(
            hex::encode(reduced),
            "000000000000000000000000000000014551231950b75fc4402da1732fc9bebe"
        );
    }
}
