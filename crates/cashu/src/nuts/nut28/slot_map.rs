//! The v3 slot map (NUT-28, nutroot secrets)
//!
//! Slot `0` is the internal key `K`; slots `1..` are the keys of the
//! spend-info tree's leaves, in transmitted order, keys within each leaf in
//! order. A secret has at most 256 slots, one index byte each. Unlike the
//! legacy slot map there is no limit of 10.

use bitcoin::hashes::sha256::Hash as Sha256;
use bitcoin::hashes::{Hash, HashEngine};

use super::Error;
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut10::nutroot::Leaf;
use crate::nuts::nut10::nutroot::LeafBytes;
use crate::SECP256K1;

/// Most slots one secret may enumerate: the internal key plus 255 leaf keys
pub const MAX_SLOTS: usize = 256;

/// `rᵢ = SHA256("Cashu_P2BK_v1" || Zx || i)`, retried once with a trailing
/// `0xff` if out of range
///
/// `Zx` is the x-coordinate of `secret_key * point`: `e·P` for the sender,
/// `p·E` for the receiver.
pub fn slot_scalar(
    secret_key: &SecretKey,
    point: &PublicKey,
    slot: u8,
) -> Result<SecretKey, Error> {
    let shared = point.mul_tweak(&SECP256K1, &secret_key.as_scalar())?;
    let zx = shared.x_only_public_key().serialize();

    let mut engine = Sha256::engine();
    engine.input(b"Cashu_P2BK_v1");
    engine.input(&zx);
    engine.input(&[slot]);
    let digest = Sha256::from_engine(engine.clone());
    if let Ok(r) = SecretKey::from_slice(digest.as_byte_array()) {
        return Ok(r);
    }
    engine.input(&[0xff]);
    Ok(SecretKey::from_slice(
        Sha256::from_engine(engine).as_byte_array(),
    )?)
}

/// The sender blinds receiver key `key` at `slot`: `P + rᵢG` with ephemeral `e`
pub fn blind_slot(ephemeral: &SecretKey, key: &PublicKey, slot: u8) -> Result<PublicKey, Error> {
    let r = slot_scalar(ephemeral, key, slot)?;
    Ok(key.combine(&r.public_key())?)
}

/// The receiver's private key for `slot`: `(p + rᵢ) mod n`
///
/// v3 keys are compressed with their parity, so `p·G` is the key the sender
/// blinded and the standard derivation applies.
pub fn derive_slot_key(
    secret_key: &SecretKey,
    ephemeral: &PublicKey,
    slot: u8,
) -> Result<SecretKey, Error> {
    let r = slot_scalar(secret_key, ephemeral, slot)?;
    Ok(secret_key.add_tweak(&r.as_scalar())?)
}

/// Slot index of the `n`th enumerated key, refusing one past the index byte
pub fn slot_index(n: usize) -> Result<u8, Error> {
    u8::try_from(n).map_err(|_| Error::TooManySlots(n + 1))
}

/// The leaf keys of a tree in slot order: slot `1` first
///
/// Leaves that do not parse as core leaves contribute no keys.
pub fn leaf_slot_keys(tree: &[LeafBytes]) -> Result<Vec<PublicKey>, Error> {
    let keys: Vec<PublicKey> = tree
        .iter()
        .filter_map(|leaf| leaf.parse_known().ok())
        .flat_map(|leaf: Leaf| leaf.keys().to_vec())
        .collect();
    if keys.len() + 1 > MAX_SLOTS {
        return Err(Error::TooManySlots(keys.len() + 1));
    }
    Ok(keys)
}

/// Find the receiver's slots among a tree's leaf keys, matching by value
///
/// For each slot `1..=N` the receiver derives its candidate key and keeps
/// the slots whose candidate a leaf lists. Returns `(slot, private key)`.
pub fn match_leaf_slots(
    secret_key: &SecretKey,
    ephemeral: &PublicKey,
    tree: &[LeafBytes],
) -> Result<Vec<(u8, SecretKey)>, Error> {
    let keys = leaf_slot_keys(tree)?;
    let mut matches = Vec::new();
    for n in 1..=keys.len() {
        let slot = slot_index(n)?;
        let candidate = derive_slot_key(secret_key, ephemeral, slot)?;
        if keys.contains(&candidate.public_key()) {
            matches.push((slot, candidate));
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::util::hex;

    fn key(n: u8) -> SecretKey {
        SecretKey::from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap()
    }

    /// tests/28-tests.md "Nutroot secrets (v3): the slot map"
    #[test]
    fn slot_map_vector() {
        let (receiver, owner, ephemeral) = (key(3), key(4), key(5));
        assert_eq!(
            hex::encode(
                slot_scalar(&ephemeral, &receiver.public_key(), 0)
                    .unwrap()
                    .to_secret_bytes()
            ),
            "7dfb649b0edda814f7cf0feb889e5657eb2083a528aa60a3a943fe0cea066181"
        );
        let slot0 = blind_slot(&ephemeral, &receiver.public_key(), 0).unwrap();
        assert_eq!(
            slot0.to_hex(),
            "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51"
        );
        let slot1 = blind_slot(&ephemeral, &owner.public_key(), 1).unwrap();
        assert_eq!(
            slot1.to_hex(),
            "039ca57991c48db95252bff61e02c31cf9b1e9ec2ef27d9dee33db6f0324e6ca81"
        );

        // Receivers derive the matching private keys from E.
        let e_pub = ephemeral.public_key();
        assert_eq!(
            derive_slot_key(&receiver, &e_pub, 0).unwrap().public_key(),
            slot0
        );
        assert_eq!(
            derive_slot_key(&owner, &e_pub, 1).unwrap().public_key(),
            slot1
        );

        // The owner of key 4 finds slot 1 by value.
        let leaf = LeafBytes::from_str("000202000101040021039ca57991c48db95252bff61e02c31cf9b1e9ec2ef27d9dee33db6f0324e6ca8106000468a3be80").unwrap();
        let found = match_leaf_slots(&owner, &e_pub, std::slice::from_ref(&leaf)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, 1);
        assert_eq!(found[0].1.public_key(), slot1);
        // The receiver's key is not in the tree.
        assert!(match_leaf_slots(&receiver, &e_pub, &[leaf])
            .unwrap()
            .is_empty());
    }

    /// The legacy vectors' r scalars and blinded keys hold for the v3 slot
    /// derivation too (same KDF), past the legacy limit of 10 slots
    #[test]
    fn legacy_vector_scalars() {
        let e =
            SecretKey::from_hex("1cedb9df0c6872188b560ace9e35fd55c2532d53e19ae65b46159073886482ca")
                .unwrap();
        let p = PublicKey::from_hex(
            "02771fed6cb88aaac38b8b32104a942bf4b8f4696bc361171b3c7d06fa2ebddf06",
        )
        .unwrap();
        assert_eq!(
            hex::encode(slot_scalar(&e, &p, 10).unwrap().to_secret_bytes()),
            "9de35112d62e6343d02301d8f58fef87958e99bb68cfdfa855e04fe18b95b114"
        );
        assert_eq!(
            blind_slot(&e, &p, 10).unwrap().to_hex(),
            "032371fc0eef6885062581a3852494e2eab8f384b7dd196281b85b77f94770fac5"
        );
        slot_scalar(&e, &p, 255).unwrap();
    }

    #[test]
    fn slot_index_fits_one_byte() {
        assert_eq!(slot_index(255).unwrap(), 255);
        assert!(matches!(slot_index(256), Err(Error::TooManySlots(257))));
    }
}
