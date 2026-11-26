//! Internal keys
//!
//! `K` is a single-party key, an aggregated key (which carries at least the
//! empty tweak), or a NUMS offset key `K = H + u*G` that makes the secret
//! script-only.

use bitcoin::secp256k1::Scalar;

use super::Error;
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::SECP256K1;

/// BIP341's NUMS point `H = lift_x(SHA256(G_uncompressed))`
pub const NUMS_H: &str = "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

/// The NUMS point `H`
///
/// # Panics
///
/// Never: [`NUMS_H`] is a valid point, as a unit test checks.
pub fn nums_h() -> PublicKey {
    PublicKey::from_hex(NUMS_H).expect("NUMS_H is a valid point")
}

/// The NUMS offset internal key `K = H + u*G`
pub fn nums_internal_key(u: &SecretKey) -> Result<PublicKey, Error> {
    Ok(nums_h().combine(&u.public_key())?)
}

/// Whether `K - u*G == H`
pub fn is_nums_internal_key(internal_key: &PublicKey, u: &SecretKey) -> bool {
    let neg_u_g = u.public_key().negate(&SECP256K1);
    internal_key
        .combine(&neg_u_g)
        .is_ok_and(|point| point == nums_h())
}

/// `K + s*G` for a scalar `s`
pub(crate) fn add_scalar_g(point: &PublicKey, scalar: &Scalar) -> Result<PublicKey, Error> {
    Ok(PublicKey::from(
        point
            .add_exp_tweak(&SECP256K1, scalar)
            .map_err(|_| Error::InvalidTweak)?,
    ))
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::sha256::Hash as Sha256Hash;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Parity, XOnlyPublicKey};

    use super::*;

    #[test]
    fn nums_h_is_lift_x_of_hashed_generator() {
        let one = SecretKey::from_slice(&[[0u8; 31].as_slice(), &[1]].concat()).unwrap();
        let g_uncompressed = one.public_key().to_uncompressed_bytes();
        let x = Sha256Hash::hash(&g_uncompressed).to_byte_array();
        let lifted = bitcoin::secp256k1::PublicKey::from_x_only_public_key(
            XOnlyPublicKey::from_slice(&x).unwrap(),
            Parity::Even,
        );
        assert_eq!(PublicKey::from(lifted), nums_h());
    }

    #[test]
    fn nums_offset_round_trips() {
        // tests/10 commit leaf example: u = 7
        let u = SecretKey::from_slice(&[[0u8; 31].as_slice(), &[7]].concat()).unwrap();
        let k = nums_internal_key(&u).unwrap();
        assert_eq!(
            k.to_hex(),
            "028edfebd6fdea3e1d89359af20868a2e76315b36cdb1a79de497a1757ca7bd407"
        );
        assert!(is_nums_internal_key(&k, &u));
        let other = SecretKey::from_slice(&[[0u8; 31].as_slice(), &[8]].concat()).unwrap();
        assert!(!is_nums_internal_key(&k, &other));
    }
}
