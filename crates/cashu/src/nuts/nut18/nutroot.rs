//! The `nutroot` payment request option (NUT-18, v3 keysets)
//!
//! ```json
//! { "k": "<receiver key>", "l": ["<leaf>", ...], "b": ["<blind-me key>", ...] }
//! ```
//!
//! Every output's secret derives from `k` per NUT-28: the payer blinds `k`
//! at slot 0 into the internal key (or offsets the NUMS point by a fresh
//! `u`), reproduces the requested leaves byte for byte with the `b` keys
//! slot-blinded, and tweaks the internal key by the tree root.

use serde::{Deserialize, Serialize};

use super::Error;
use crate::nuts::nut00::spend_info::{Reconstruction, SpendInfo};
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut10::nutroot::internal_key::nums_h;
use crate::nuts::nut10::nutroot::{
    merkle_root, nums_internal_key, tweak, tweaked_key, Leaf, LeafBytes,
};
use crate::nuts::nut28::slot_map::{blind_slot, slot_index, MAX_SLOTS};

/// The `nutroot` option of a payment request
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NutrootOption {
    /// The payee's static receiver key, or the NUMS point for script-only
    /// payments
    pub k: PublicKey,
    /// The requested condition tree, in slot order
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<Vec<LeafBytes>>,
    /// Keys of `l` their owners tag blind-me
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<Vec<PublicKey>>,
}

/// One output's lock, built by the payer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NutrootPayment {
    /// The output's secret point
    pub secret: PublicKey,
    /// The spend info to send with the proof
    pub spend_info: SpendInfo,
}

impl NutrootOption {
    /// Whether `k` is the NUMS point: payments are script-only
    pub fn is_nums(&self) -> bool {
        self.k == nums_h()
    }

    fn blind_me(&self) -> &[PublicKey] {
        self.b.as_deref().unwrap_or_default()
    }

    fn requested_leaves(&self) -> Result<Vec<Leaf>, Error> {
        self.l
            .iter()
            .flatten()
            .map(|bytes| {
                let leaf = bytes.parse_known().map_err(|_| Error::NutrootLeaf)?;
                // A leaf the payer cannot reproduce byte for byte is refused.
                if leaf.to_leaf_bytes().ok().as_ref() != Some(bytes) {
                    return Err(Error::NutrootLeaf);
                }
                Ok(leaf)
            })
            .collect()
    }

    /// Build one output's lock
    ///
    /// `ephemeral` is a fresh keypair for this output and `u` a fresh NUMS
    /// offset, used only for a NUMS `k`. The ephemeral travels only if it
    /// blinded something.
    pub fn pay(&self, ephemeral: &SecretKey, u: &SecretKey) -> Result<NutrootPayment, Error> {
        let nums = self.is_nums();
        let requested = self.requested_leaves()?;
        if nums && requested.is_empty() {
            // A script-only proof with no leaves can never be spent.
            return Err(Error::NutrootNoLeaves);
        }

        let internal_key = match nums {
            true => nums_internal_key(u).map_err(|_| Error::NutrootLeaf)?,
            false => blind_slot(ephemeral, &self.k, 0)?,
        };

        let mut slot = 0usize;
        let mut blinded_any = !nums;
        let mut tree = Vec::with_capacity(requested.len());
        for leaf in requested {
            let keys = leaf
                .keys()
                .iter()
                .map(|key| {
                    slot += 1;
                    if slot >= MAX_SLOTS {
                        return Err(Error::Nut28(crate::nuts::nut28::Error::TooManySlots(
                            slot + 1,
                        )));
                    }
                    match self.blind_me().contains(key) {
                        true => {
                            blinded_any = true;
                            Ok(blind_slot(ephemeral, key, slot_index(slot)?)?)
                        }
                        false => Ok(*key),
                    }
                })
                .collect::<Result<Vec<_>, Error>>()?;
            tree.push(
                with_keys(&leaf, keys)
                    .to_leaf_bytes()
                    .map_err(|_| Error::NutrootLeaf)?,
            );
        }

        let secret = match tree.is_empty() {
            true => internal_key,
            false => {
                let root = merkle_root(&tree).map_err(|_| Error::NutrootLeaf)?;
                tweaked_key(&internal_key, &tweak(&internal_key, Some(&root)))
                    .map_err(|_| Error::NutrootLeaf)?
            }
        };

        let mut spend_info = SpendInfo::default();
        spend_info.e = blinded_any.then(|| ephemeral.public_key());
        spend_info.internal_key = (!tree.is_empty()).then_some(internal_key);
        spend_info.u = nums.then(|| u.clone());
        spend_info.tree = (!tree.is_empty()).then_some(tree);
        Ok(NutrootPayment { secret, spend_info })
    }

    /// The payee's check of a received proof against this request
    ///
    /// Runs receive check 1, then requires `E` exactly when the request
    /// blinds something, `u` exactly for a NUMS `k`, and the disclosed tree
    /// to match `l` one-to-one in any order, each `b` key replaced in place
    /// by some point.
    pub fn verify_payment(
        &self,
        secret: &PublicKey,
        spend_info: &SpendInfo,
        receiver_key: Option<&SecretKey>,
    ) -> Result<Reconstruction, Error> {
        let nums = self.is_nums();
        let expects_e = !nums || !self.blind_me().is_empty();
        if spend_info.e.is_some() != expects_e {
            return Err(Error::NutrootMismatch("E presence"));
        }
        if spend_info.u.is_some() != nums {
            return Err(Error::NutrootMismatch("u presence"));
        }
        if spend_info.k.is_some() {
            return Err(Error::NutrootMismatch("bearer key"));
        }

        let reconstruction = spend_info.reconstruct(secret, receiver_key)?;
        if !nums && reconstruction.script_only {
            return Err(Error::NutrootMismatch("script-only"));
        }

        let requested = self.requested_leaves()?;
        if !exact_tree(&requested, &reconstruction.leaves, self.blind_me()) {
            return Err(Error::NutrootMismatch("tree"));
        }
        Ok(reconstruction)
    }
}

fn with_keys(leaf: &Leaf, keys: Vec<PublicKey>) -> Leaf {
    match leaf.clone() {
        Leaf::Threshold { n, .. } => Leaf::Threshold { n, keys },
        Leaf::After { n, time, .. } => Leaf::After { n, keys, time },
        Leaf::Hashlock { n, hash, .. } => Leaf::Hashlock { n, keys, hash },
        commit @ Leaf::Commit { .. } => commit,
    }
}

/// Whether `disclosed` matches `requested` with each blind-me key replaced
fn leaf_matches(requested: &Leaf, disclosed: &Leaf, blind_me: &[PublicKey]) -> bool {
    if requested.keys().len() != disclosed.keys().len() {
        return false;
    }
    let keys = requested
        .keys()
        .iter()
        .zip(disclosed.keys())
        .map(
            |(requested, disclosed)| match blind_me.contains(requested) {
                true => *disclosed,
                false => *requested,
            },
        )
        .collect();
    with_keys(requested, keys) == *disclosed
}

/// One-to-one, order-insensitive match of the disclosed tree to the request
fn exact_tree(requested: &[Leaf], disclosed: &[Leaf], blind_me: &[PublicKey]) -> bool {
    fn assign(
        requested: &[Leaf],
        disclosed: &[Leaf],
        blind_me: &[PublicKey],
        used: &mut Vec<bool>,
    ) -> bool {
        let Some((first, rest)) = requested.split_first() else {
            return true;
        };
        for (index, candidate) in disclosed.iter().enumerate() {
            if !used[index] && leaf_matches(first, candidate, blind_me) {
                used[index] = true;
                if assign(rest, disclosed, blind_me, used) {
                    return true;
                }
                used[index] = false;
            }
        }
        false
    }
    requested.len() == disclosed.len()
        && assign(
            requested,
            disclosed,
            blind_me,
            &mut vec![false; disclosed.len()],
        )
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const AFTER_LEAF: &str = "00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80";
    const BLINDED_LEAF: &str = "000202000101040021039ca57991c48db95252bff61e02c31cf9b1e9ec2ef27d9dee33db6f0324e6ca8106000468a3be80";
    const KEY4: &str = "02e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13";

    fn key(n: u8) -> SecretKey {
        SecretKey::from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap()
    }

    fn request(k: PublicKey, blind: bool) -> NutrootOption {
        NutrootOption {
            k,
            l: Some(vec![LeafBytes::from_str(AFTER_LEAF).unwrap()]),
            b: blind.then(|| vec![PublicKey::from_hex(KEY4).unwrap()]),
        }
    }

    /// tests/18-tests.md "Nutroot Locking"
    #[test]
    fn receiver_keyed_request() {
        let option = request(key(3).public_key(), true);
        let payment = option.pay(&key(5), &key(1)).unwrap();
        // The vector pins K and the blinded leaf (the NUT-28 slot-map vectors).
        assert_eq!(
            payment.spend_info.internal_key.unwrap().to_hex(),
            "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51"
        );
        assert_eq!(
            payment.spend_info.tree.as_ref().unwrap()[0].to_string(),
            BLINDED_LEAF
        );
        assert_eq!(payment.spend_info.e, Some(key(5).public_key()));
        assert!(payment.spend_info.u.is_none());

        let reconstruction = option
            .verify_payment(&payment.secret, &payment.spend_info, Some(&key(3)))
            .unwrap();
        assert!(reconstruction.key_path_key.is_some());
    }

    /// tests/18-tests.md NUMS request without `b`: no ephemeral travels
    #[test]
    fn nums_request_without_blinding() {
        let option = request(nums_h(), false);
        let payment = option.pay(&key(5), &key(9)).unwrap();
        assert_eq!(
            payment.secret.to_hex(),
            "030b5dc180dd2ef76be0f0319fc5c254511fede8c8563a823d3b0fcd6f9012a0b2"
        );
        assert_eq!(
            payment.spend_info.internal_key.unwrap().to_hex(),
            "03b948fab26606a34380c4515ece4c27d25fcf53eb95d1041630ab44f2be4f7331"
        );
        assert!(payment.spend_info.e.is_none());
        assert_eq!(
            payment.spend_info.tree.as_ref().unwrap()[0].to_string(),
            AFTER_LEAF
        );
        option
            .verify_payment(&payment.secret, &payment.spend_info, None)
            .unwrap();

        // An ephemeral that blinded nothing is rejected.
        let mut with_e = payment.spend_info.clone();
        with_e.e = Some(key(5).public_key());
        assert!(option
            .verify_payment(&payment.secret, &with_e, None)
            .is_err());
    }

    #[test]
    fn nums_without_leaves_is_refused() {
        let option = NutrootOption {
            k: nums_h(),
            l: None,
            b: None,
        };
        assert!(matches!(
            option.pay(&key(5), &key(9)),
            Err(Error::NutrootNoLeaves)
        ));
    }

    #[test]
    fn exact_tree_rules() {
        let option = request(key(3).public_key(), true);
        let payment = option.pay(&key(5), &key(1)).unwrap();

        // A tree with an extra leaf is not the requested tree.
        let mut extra = payment.spend_info.clone();
        let threshold = Leaf::Threshold {
            n: 1,
            keys: vec![key(9).public_key()],
        }
        .to_leaf_bytes()
        .unwrap();
        extra.tree.as_mut().unwrap().push(threshold);
        let root = merkle_root(extra.tree.as_ref().unwrap()).unwrap();
        let k = extra.internal_key.unwrap();
        let secret = tweaked_key(&k, &tweak(&k, Some(&root))).unwrap();
        assert!(matches!(
            option.verify_payment(&secret, &extra, Some(&key(3))),
            Err(Error::NutrootMismatch("tree"))
        ));

        // A key not tagged blind-me must come back verbatim.
        let verbatim = request(key(3).public_key(), false);
        assert!(matches!(
            verbatim.verify_payment(&payment.secret, &payment.spend_info, Some(&key(3))),
            Err(Error::NutrootMismatch("tree"))
        ));
    }

    #[test]
    fn json_round_trip() {
        let json = serde_json::json!({
            "k": "02f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
            "l": [AFTER_LEAF],
            "b": [KEY4],
        });
        let option: NutrootOption = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&option).unwrap(), json);
    }

    /// tests/18-tests.md NUMS request with `b`: the payment the spec lists
    #[test]
    fn nums_request_with_blinding_vector() {
        let option = request(nums_h(), true);
        let payment = option.pay(&key(5), &key(7)).unwrap();
        assert_eq!(
            payment.secret.to_hex(),
            "02fb23814e330739413a6e3982a21916002962002bc101ac105a28e0cf3bcb46d1"
        );
        let expected: SpendInfo = serde_json::from_value(serde_json::json!({
            "E": "022f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4",
            "K": "028edfebd6fdea3e1d89359af20868a2e76315b36cdb1a79de497a1757ca7bd407",
            "u": "0000000000000000000000000000000000000000000000000000000000000007",
            "tree": [BLINDED_LEAF],
        }))
        .unwrap();
        assert_eq!(payment.spend_info, expected);
        // The payee (no receiver key needed under NUMS) accepts it.
        option
            .verify_payment(&payment.secret, &payment.spend_info, None)
            .unwrap();
    }

    /// tests/18-tests.md: the NUMS request's creqA encoding
    #[test]
    fn nums_request_creq_a_vector() {
        use std::str::FromStr;

        use crate::nuts::nut18::PaymentRequest;

        let encoded = "creqAo2FhCGF1Y3NhdGdudXRyb290o2FreEIwMjUwOTI5Yjc0YzFhMDQ5NTRiNzhiNGI2MDM1ZTk3YTVlMDc4YTVhMGYyOGVjOTZkNTQ3YmZlZTlhY2U4MDNhYzBhbIF4YjAwMDIwMjAwMDEwMTA0MDAyMTAyZTQ5M2RiZjFjMTBkODBmMzU4MWU0OTA0OTMwYjE0MDRjYzZjMTM5MDBlZTA3NTg0NzRmYTk0YWJlOGM0Y2QxMzA2MDAwNDY4YTNiZTgwYWKBeEIwMmU0OTNkYmYxYzEwZDgwZjM1ODFlNDkwNDkzMGIxNDA0Y2M2YzEzOTAwZWUwNzU4NDc0ZmE5NGFiZThjNGNkMTM=";
        let decoded = PaymentRequest::from_str(encoded).unwrap();
        assert_eq!(decoded.nutroot, Some(request(nums_h(), true)));
        assert_eq!(decoded.to_string(), encoded);
    }
}
