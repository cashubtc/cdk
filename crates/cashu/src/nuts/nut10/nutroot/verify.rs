//! Witness verification
//!
//! The key path is one BIP-340 signature by the secret's x-coordinate. The
//! script path recomputes the commitment from the revealed leaf, path and
//! internal key, then evaluates the leaf.

use std::collections::HashSet;

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use bitcoin::secp256k1::Message;

use super::leaf::Leaf;
use super::tree::root_from_path;
use super::tweak::{tweak, tweaked_key};
use super::witness::NutrootWitness;
use super::Error;
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::SECP256K1;

/// Verify a witness for `secret` over a 32-byte digest
///
/// `secret` is the proof's secret point, or a mint quote's lock key. `digest`
/// is the input digest; `now` is the verifier's clock in unix seconds, used
/// by `after` leaves.
pub fn verify_witness(
    secret: &PublicKey,
    witness: &NutrootWitness,
    digest: &[u8; 32],
    now: u64,
) -> Result<(), Error> {
    let message = Message::from_digest(*digest);
    let Some(leaf_bytes) = &witness.leaf else {
        if witness.control.is_some() || witness.preimage.is_some() {
            return Err(Error::MalformedWitness(
                "control and preimage need a revealed leaf",
            ));
        }
        let [signature] = witness.signatures.as_slice() else {
            return Err(Error::MalformedWitness(
                "a key-path witness has exactly one signature",
            ));
        };
        return verify_signature(secret, signature, &message)
            .then_some(())
            .ok_or(Error::InvalidSignature);
    };

    let control = witness.control.as_ref().ok_or(Error::MalformedWitness(
        "a script-path witness needs control",
    ))?;

    // 1-2: path length, then the commitment to the secret.
    let root = root_from_path(&leaf_bytes.hash(), &control.path)?;
    let t = tweak(&control.internal_key, Some(&root));
    if tweaked_key(&control.internal_key, &t)? != *secret {
        return Err(Error::CommitmentMismatch);
    }

    // 3: parse, failing closed on unknown versions, types and fields.
    let leaf = leaf_bytes.parse_known()?;

    // 4: evaluate.
    if witness.preimage.is_some() && !matches!(leaf, Leaf::Hashlock { .. }) {
        return Err(Error::MalformedWitness("preimage on a non-hashlock leaf"));
    }
    match &leaf {
        Leaf::Commit { .. } => return Err(Error::CommitLeafRevealed),
        Leaf::After { time, .. } if now < *time => return Err(Error::LocktimeNotReached),
        Leaf::Hashlock { hash, .. } => {
            let preimage = witness.preimage.as_ref().ok_or(Error::InvalidPreimage)?;
            if preimage.len() != 32 || Sha256Hash::hash(preimage).to_byte_array() != *hash {
                return Err(Error::InvalidPreimage);
            }
        }
        _ => (),
    }

    let keys = leaf.keys();
    if witness.signatures.len() > keys.len() {
        return Err(Error::MalformedWitness(
            "more signatures than the leaf lists keys",
        ));
    }
    let required = usize::from(leaf.threshold().unwrap_or_default());
    let mut satisfied = HashSet::new();
    for signature in &witness.signatures {
        for key in keys {
            if verify_signature(key, signature, &message) {
                satisfied.insert(key.x_only_public_key().serialize());
            }
        }
    }
    if satisfied.len() < required {
        return Err(Error::SignaturesNotMet {
            valid: satisfied.len(),
            required,
        });
    }
    Ok(())
}

fn verify_signature(key: &PublicKey, signature: &Signature, message: &Message) -> bool {
    SECP256K1
        .verify_schnorr(signature, message, &key.x_only_public_key())
        .is_ok()
}

/// Sign a 32-byte digest with BIP-340 (fresh auxiliary randomness)
pub fn sign_digest(secret_key: &SecretKey, digest: &[u8; 32]) -> Signature {
    let keypair =
        bitcoin::secp256k1::Keypair::from_secret_key(&SECP256K1, secret_key.as_secp256k1());
    SECP256K1.sign_schnorr(&Message::from_digest(*digest), &keypair)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::nut10::nutroot::leaf::LeafBytes;
    use crate::nuts::nut10::nutroot::tree::{merkle_path, merkle_root};
    use crate::nuts::nut10::nutroot::tweak::tweaked_secret_key;
    use crate::nuts::nut10::nutroot::witness::Control;
    use crate::nuts::nut10::nutroot::{nums_internal_key, Tweak};
    use crate::util::hex;

    const ILLUSTRATIVE_DIGEST: &str =
        "e1d7170b89a2b6eedec90453e32b6c320dfadd590e6a6454bddec95a0e3834cd";
    const AFTER_1OF1_KEY4: &str = "00020200010104002102e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd1306000468a3be80";
    const AFTER_TIME: u64 = 1_755_561_600;
    const THRESHOLD_1OF1_KEY3: &str =
        "00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9";
    const HASHLOCK_1OF1_KEY3: &str = "00030200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9080020a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

    fn key(n: u8) -> SecretKey {
        SecretKey::from_slice(&[[0u8; 31].as_slice(), &[n]].concat()).unwrap()
    }

    fn digest(hex_digest: &str) -> [u8; 32] {
        hex::decode(hex_digest).unwrap().try_into().unwrap()
    }

    fn leaf(hex: &str) -> LeafBytes {
        LeafBytes::from_str(hex).unwrap()
    }

    fn witness(json: &str) -> NutrootWitness {
        NutrootWitness::from_str(json).unwrap()
    }

    /// Sign a digest with zero auxiliary randomness, as the vectors do
    fn sign_zero_aux(secret_key: &SecretKey, digest: &[u8; 32]) -> Signature {
        let keypair =
            bitcoin::secp256k1::Keypair::from_secret_key(&SECP256K1, secret_key.as_secp256k1());
        SECP256K1.sign_schnorr_with_aux_rand(&Message::from_digest(*digest), &keypair, &[0; 32])
    }

    /// tests/10-tests.md "Worked example: receiver-keyed proof with a refund leaf"
    mod worked_example {
        use super::*;

        const SECRET: &str = "02d310a4d661e3158e7d360617e739d6bacbf015431b24a43168db0ab99ef8f828";
        const INTERNAL_KEY: &str =
            "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51";
        const CAROL_SIG: &str = "619e0726595b5adff06cc3e6ea1c409f10f7b064cf8888f0eed0efbac854eabf4632642930bdc7c4d7d983379301a4f263991dbd19d96e5ebcfab9e8583bd510";
        const ALICE_SIG: &str = "0b2ea247bfca1264db86907aef4cb19935ed9a5b2043a757259dcdb5c599372c230602f2cfc0cd8b11aa98ff17fbed91f38f07db817263fcc7c50c846715873e";

        fn secret() -> PublicKey {
            PublicKey::from_hex(SECRET).unwrap()
        }

        fn script_path(signatures: &[&str]) -> NutrootWitness {
            witness(&format!(
                r#"{{"leaf":"{AFTER_1OF1_KEY4}","control":{{"K":"{INTERNAL_KEY}","path":[]}},"signatures":[{}]}}"#,
                signatures
                    .iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        }

        #[test]
        fn commitment() {
            let internal_key = PublicKey::from_hex(INTERNAL_KEY).unwrap();
            let root = merkle_root(&[leaf(AFTER_1OF1_KEY4)]).unwrap();
            assert_eq!(
                hex::encode(root),
                "9ed9c0b8907f7af4fce51cbeac218907bbf80ba40f3342df2406bce30616589a"
            );
            let t = tweak(&internal_key, Some(&root));
            assert_eq!(
                hex::encode(t.to_bytes()),
                "b3b7846b14be0650bb03272d179931221637744d1997df80bf27d2df5effe8a4"
            );
            assert_eq!(tweaked_key(&internal_key, &t).unwrap(), secret());
        }

        #[test]
        fn carol_key_path() {
            let keypath_priv = SecretKey::from_hex(
                "31b2e906239bae65b2d23718a037877b46a91b0b92f99fe8a899725f78d008e7",
            )
            .unwrap();
            assert_eq!(keypath_priv.public_key(), secret());
            let d = digest(ILLUSTRATIVE_DIGEST);
            assert_eq!(
                hex::encode(sign_zero_aux(&keypath_priv, &d).serialize()),
                CAROL_SIG
            );
            let carol = witness(&format!(r#"{{"signatures":["{CAROL_SIG}"]}}"#));
            verify_witness(&secret(), &carol, &d, 0).unwrap();
            // A fresh signature verifies too.
            verify_witness(
                &secret(),
                &NutrootWitness::key_path(sign_digest(&keypath_priv, &d)),
                &d,
                0,
            )
            .unwrap();

            // Listed twice: invalid.
            let twice = witness(&format!(
                r#"{{"signatures":["{CAROL_SIG}","{CAROL_SIG}"]}}"#
            ));
            assert!(matches!(
                verify_witness(&secret(), &twice, &d, 0),
                Err(Error::MalformedWitness(_))
            ));
            // Another digest: invalid.
            assert!(matches!(
                verify_witness(&secret(), &carol, &[0; 32], 0),
                Err(Error::InvalidSignature)
            ));
        }

        #[test]
        fn alice_script_path() {
            let d = digest(ILLUSTRATIVE_DIGEST);
            assert_eq!(
                hex::encode(sign_zero_aux(&key(4), &d).serialize()),
                ALICE_SIG
            );
            let alice = script_path(&[ALICE_SIG]);
            verify_witness(&secret(), &alice, &d, AFTER_TIME).unwrap();
            verify_witness(&secret(), &alice, &d, AFTER_TIME + 1).unwrap();
            assert!(matches!(
                verify_witness(&secret(), &alice, &d, AFTER_TIME - 1),
                Err(Error::LocktimeNotReached)
            ));
            // Listed twice, while the leaf lists one key: invalid.
            assert!(matches!(
                verify_witness(
                    &secret(),
                    &script_path(&[ALICE_SIG, ALICE_SIG]),
                    &d,
                    AFTER_TIME
                ),
                Err(Error::MalformedWitness(_))
            ));
            // Wrong secret: the commitment does not match.
            assert!(matches!(
                verify_witness(&key(9).public_key(), &alice, &d, AFTER_TIME),
                Err(Error::CommitmentMismatch)
            ));
            // A preimage on an after leaf: invalid.
            let mut with_preimage = alice.clone();
            with_preimage.preimage = Some(vec![0; 32]);
            assert!(matches!(
                verify_witness(&secret(), &with_preimage, &d, AFTER_TIME),
                Err(Error::MalformedWitness(_))
            ));
            // Carol's key-path signature does not satisfy the leaf.
            assert!(matches!(
                verify_witness(&secret(), &script_path(&[CAROL_SIG]), &d, AFTER_TIME),
                Err(Error::SignaturesNotMet {
                    valid: 0,
                    required: 1
                })
            ));
        }
    }

    /// tests/10-tests.md three-leaf tree: internal key and secret
    #[test]
    fn three_leaf_secret_and_script_path() {
        let leaves = [
            leaf(THRESHOLD_1OF1_KEY3),
            leaf(AFTER_1OF1_KEY4),
            leaf(HASHLOCK_1OF1_KEY3),
        ];
        let internal_key = PublicKey::from_hex(
            "03a3e12cc077e5605f36441046f50c114fcc883b079a34028bed66732e3a419e51",
        )
        .unwrap();
        let root = merkle_root(&leaves).unwrap();
        let secret = tweaked_key(&internal_key, &tweak(&internal_key, Some(&root))).unwrap();
        assert_eq!(
            secret.to_hex(),
            "022d17fddb224e53e12b40c58ab3e8828d08931640105c52fc4eaf765ed51b9999"
        );

        // Spend the hashlock leaf (index 2) through its path.
        let d = [7u8; 32];
        let mut hashlock_witness = NutrootWitness {
            signatures: vec![sign_digest(&key(3), &d)],
            leaf: Some(leaves[2].clone()),
            control: Some(Control {
                internal_key,
                path: merkle_path(&leaves, 2).unwrap(),
            }),
            preimage: None,
        };
        assert!(matches!(
            verify_witness(&secret, &hashlock_witness, &d, 0),
            Err(Error::InvalidPreimage)
        ));
        // The vector's hash is not the SHA256 of any known preimage; check
        // the length rule and the mismatch.
        hashlock_witness.preimage = Some(vec![0xa1; 31]);
        assert!(matches!(
            verify_witness(&secret, &hashlock_witness, &d, 0),
            Err(Error::InvalidPreimage)
        ));
        hashlock_witness.preimage = Some(vec![0xa1; 32]);
        assert!(matches!(
            verify_witness(&secret, &hashlock_witness, &d, 0),
            Err(Error::InvalidPreimage)
        ));

        // The threshold leaf spends with key 3; a 4-hash path is refused.
        let mut threshold_witness = NutrootWitness {
            signatures: vec![sign_digest(&key(3), &d)],
            leaf: Some(leaves[0].clone()),
            control: Some(Control {
                internal_key,
                path: merkle_path(&leaves, 0).unwrap(),
            }),
            preimage: None,
        };
        verify_witness(&secret, &threshold_witness, &d, 0).unwrap();
        if let Some(control) = threshold_witness.control.as_mut() {
            control.path = vec![[0; 32]; 4];
        }
        assert!(matches!(
            verify_witness(&secret, &threshold_witness, &d, 0),
            Err(Error::PathTooLong(4))
        ));
    }

    #[test]
    fn hashlock_with_real_preimage() {
        let preimage = [0x42u8; 32];
        let hash = Sha256Hash::hash(&preimage).to_byte_array();
        let leaves = [Leaf::Hashlock {
            n: 1,
            keys: vec![key(3).public_key()],
            hash,
        }
        .to_leaf_bytes()
        .unwrap()];
        let internal_key = key(6).public_key();
        let root = merkle_root(&leaves).unwrap();
        let secret = tweaked_key(&internal_key, &tweak(&internal_key, Some(&root))).unwrap();
        let d = [9u8; 32];
        let mut w = NutrootWitness {
            signatures: vec![sign_digest(&key(3), &d)],
            leaf: Some(leaves[0].clone()),
            control: Some(Control {
                internal_key,
                path: vec![],
            }),
            preimage: Some(preimage.to_vec()),
        };
        verify_witness(&secret, &w, &d, 0).unwrap();
        w.signatures = vec![];
        assert!(matches!(
            verify_witness(&secret, &w, &d, 0),
            Err(Error::SignaturesNotMet {
                valid: 0,
                required: 1
            })
        ));
    }

    /// tests/10-tests.md "Worked example: a commit leaf"
    #[test]
    fn commit_leaf_vector() {
        let u = key(7);
        let internal_key = nums_internal_key(&u).unwrap();
        let commit_hash = Sha256Hash::hash(b"external data").to_byte_array();
        assert_eq!(
            hex::encode(commit_hash),
            "64451ff981aa92887b070762e97302df00a0ab97a853c323394b0a9cfaf46fae"
        );
        let commit = Leaf::Commit { hash: commit_hash }.to_leaf_bytes().unwrap();
        let threshold = leaf(THRESHOLD_1OF1_KEY3);
        assert_eq!(
            hex::encode(commit.hash()),
            "20cccc22a45ddfdbec0647032487870dbeadb03471164eb6767b1234f9914f27"
        );
        let leaves = [threshold.clone(), commit.clone()];
        let root = merkle_root(&leaves).unwrap();
        assert_eq!(
            hex::encode(root),
            "a23cd934664be4637fd2d9a16febca5ed06e7e3d0adef2231eeb7bcacaa156cd"
        );
        let t = tweak(&internal_key, Some(&root));
        assert_eq!(
            hex::encode(t.to_bytes()),
            "26992654d7ef6a6422bc84867ce87e7c641a59dacf86e2b339e434a92af374cf"
        );
        let secret = tweaked_key(&internal_key, &t).unwrap();
        assert_eq!(
            secret.to_hex(),
            "028f86ae14b1a813b7093473faf7b1024979dd6da4db0790126daa2922331afce0"
        );

        let d = [1u8; 32];
        let spend = |leaf: &LeafBytes, path: Vec<[u8; 32]>| NutrootWitness {
            signatures: vec![sign_digest(&key(3), &d)],
            leaf: Some(leaf.clone()),
            control: Some(Control { internal_key, path }),
            preimage: None,
        };
        // Key 3 spends through the threshold leaf with path [leaf_hash_commit].
        assert_eq!(merkle_path(&leaves, 0).unwrap(), vec![commit.hash()]);
        verify_witness(&secret, &spend(&threshold, vec![commit.hash()]), &d, 0).unwrap();
        // Revealing the commit leaf is rejected.
        assert!(matches!(
            verify_witness(&secret, &spend(&commit, vec![threshold.hash()]), &d, 0),
            Err(Error::CommitLeafRevealed)
        ));
    }

    /// tests/10-tests.md: a tree with an unknown leaf type still commits,
    /// but revealing that leaf is rejected
    #[test]
    fn unknown_leaf_commits_but_cannot_spend() {
        let unknown = leaf(
            "00ff0200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9",
        );
        let threshold = leaf(THRESHOLD_1OF1_KEY3);
        let leaves = [threshold.clone(), unknown.clone()];
        let internal_key = key(6).public_key();
        let root = merkle_root(&leaves).unwrap();
        let secret = tweaked_key(&internal_key, &tweak(&internal_key, Some(&root))).unwrap();
        let d = [2u8; 32];
        let spend = |leaf: &LeafBytes, path| NutrootWitness {
            signatures: vec![sign_digest(&key(3), &d)],
            leaf: Some(leaf.clone()),
            control: Some(Control { internal_key, path }),
            preimage: None,
        };
        verify_witness(&secret, &spend(&threshold, vec![unknown.hash()]), &d, 0).unwrap();
        assert!(matches!(
            verify_witness(&secret, &spend(&unknown, vec![threshold.hash()]), &d, 0),
            Err(Error::UnknownLeaf { .. })
        ));
    }

    /// Thresholds count distinct keys; signatures by unlisted keys do not count
    #[test]
    fn threshold_counts_distinct_listed_keys() {
        let keys = vec![
            key(3).public_key(),
            key(4).public_key(),
            key(5).public_key(),
        ];
        let leaves = [Leaf::Threshold { n: 2, keys }.to_leaf_bytes().unwrap()];
        let internal_key = key(6).public_key();
        let root = merkle_root(&leaves).unwrap();
        let secret = tweaked_key(&internal_key, &tweak(&internal_key, Some(&root))).unwrap();
        let d = [3u8; 32];
        let with = |signers: &[u8]| NutrootWitness {
            signatures: signers.iter().map(|n| sign_digest(&key(*n), &d)).collect(),
            leaf: Some(leaves[0].clone()),
            control: Some(Control {
                internal_key,
                path: vec![],
            }),
            preimage: None,
        };
        verify_witness(&secret, &with(&[3, 5]), &d, 0).unwrap();
        // The same key twice is one signer.
        assert!(matches!(
            verify_witness(&secret, &with(&[3, 3]), &d, 0),
            Err(Error::SignaturesNotMet {
                valid: 1,
                required: 2
            })
        ));
        // An unlisted key does not count.
        assert!(matches!(
            verify_witness(&secret, &with(&[3, 9]), &d, 0),
            Err(Error::SignaturesNotMet {
                valid: 1,
                required: 2
            })
        ));
    }

    /// Key-path spends work for a parity-odd tweaked key, and `02 || x` and
    /// `03 || x` are distinct secrets that one scalar spends
    #[test]
    fn key_path_parity() {
        let d = [4u8; 32];
        let internal = key(3);
        let t: Tweak = tweak(&internal.public_key(), None);
        let p = tweaked_secret_key(&internal, &t).unwrap();
        // Empty-tweak vector secret starts with 03.
        assert!(p.public_key().to_hex().starts_with("03"));
        let w = NutrootWitness::key_path(sign_digest(&p, &d));
        verify_witness(&p.public_key(), &w, &d, 0).unwrap();
        let mut even = p.public_key().to_bytes();
        even[0] = 0x02;
        verify_witness(&PublicKey::from_slice(&even).unwrap(), &w, &d, 0).unwrap();
    }

    #[test]
    fn key_path_rejects_control_without_leaf() {
        let d = [5u8; 32];
        let mut w = NutrootWitness::key_path(sign_digest(&key(3), &d));
        w.control = Some(Control {
            internal_key: key(3).public_key(),
            path: vec![],
        });
        assert!(matches!(
            verify_witness(&key(3).public_key(), &w, &d, 0),
            Err(Error::MalformedWitness(_))
        ));
    }
}
