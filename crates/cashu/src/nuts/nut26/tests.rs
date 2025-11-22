//! Tests for the NUT-26 implementation

use std::str::FromStr;

use crate::nuts::nut01::SecretKey;
use crate::nuts::nut02::KeySetVersion;
use crate::nuts::nut26::{blind_public_key, derive_signing_key_bip340, ecdh_kdf};
use crate::{Conditions, Id, PreMintSecrets, SigFlag, SpendingConditions};

#[test]
fn test_ecdh_kdf() {
    // Create a test key pair
    let sender_key = SecretKey::generate();
    let receiver_key = SecretKey::generate();
    let receiver_pubkey = receiver_key.public_key();

    // Create a test keyset ID with proper version byte
    let keyset_bytes = [KeySetVersion::Version01.to_byte()]
        .into_iter()
        .chain([1u8; 32])
        .collect::<Vec<_>>();
    let keyset_id = Id::from_bytes(&keyset_bytes).unwrap();

    // Test basic KDF derivation
    let blinding_scalar = ecdh_kdf(&sender_key, &receiver_pubkey, keyset_id, 0).unwrap();

    // Verify blinding scalar is not zero
    assert_ne!(blinding_scalar.to_secret_bytes(), [0u8; 32]);

    // Test canonical slot validation
    let result = ecdh_kdf(&sender_key, &receiver_pubkey, keyset_id, 11);
    assert!(result.is_err()); // Slot > 10 should fail

    // Test all valid slots
    for slot in 0..=10 {
        let result = ecdh_kdf(&sender_key, &receiver_pubkey, keyset_id, slot);
        assert!(result.is_ok());

        // Different slots should produce different blinding factors
        if slot > 0 {
            let previous = ecdh_kdf(&sender_key, &receiver_pubkey, keyset_id, slot - 1).unwrap();
            let current = result.unwrap();
            assert_ne!(previous.to_secret_bytes(), current.to_secret_bytes());
        }
    }
}

#[test]
fn test_blind_public_key() {
    // Create test keys
    let privkey = SecretKey::generate();
    let pubkey = privkey.public_key();
    let blinding_scalar = SecretKey::generate();

    // Blind the public key
    let blinded_pubkey = blind_public_key(&pubkey, &blinding_scalar).unwrap();

    // Verify the blinded pubkey is different
    assert_ne!(pubkey.to_string(), blinded_pubkey.to_string());

    // Verify that blinding is deterministic
    let blinded_again = blind_public_key(&pubkey, &blinding_scalar).unwrap();
    assert_eq!(blinded_pubkey.to_string(), blinded_again.to_string());

    // Verify that the result of P + r*G is correct
    // Compute r*G
    let r_pubkey = blinding_scalar.public_key();

    // Manually compute P + r*G using the library's combine function
    let combined = pubkey.combine(&r_pubkey).unwrap();

    // Verify that our blind_public_key function produces the same result
    assert_eq!(blinded_pubkey.to_string(), combined.to_string());
}

#[test]
fn test_signing_key_derivation() {
    // Create test keys
    let privkey = SecretKey::generate();
    let pubkey = privkey.public_key();
    let blinding_scalar = SecretKey::generate();

    // Blind the public key
    let blinded_pubkey = blind_public_key(&pubkey, &blinding_scalar).unwrap();

    // Derive BIP340 signing key
    let signing_key_bip340 =
        derive_signing_key_bip340(&privkey, &blinding_scalar, &blinded_pubkey).unwrap();

    // Verify that the BIP340 signing key's public key matches the blinded pubkey
    let signing_pubkey_bip340 = signing_key_bip340.public_key();
    assert_eq!(
        signing_pubkey_bip340.x_only_public_key(),
        blinded_pubkey.x_only_public_key()
    );
}

#[test]
fn test_multi_key_blinding() {
    // Create a set of keys
    let primary_key = SecretKey::generate();
    let primary_pubkey = primary_key.public_key();

    // Create additional pubkeys
    let additional_key1 = SecretKey::generate();
    let additional_key2 = SecretKey::generate();
    let additional_pubkey1 = additional_key1.public_key();
    let additional_pubkey2 = additional_key2.public_key();

    // Create refund keys
    let refund_key1 = SecretKey::generate();
    let refund_key2 = SecretKey::generate();
    let refund_pubkey1 = refund_key1.public_key();
    let refund_pubkey2 = refund_key2.public_key();

    // Create a ephemeral key for the blinding
    let ephemeral_key = SecretKey::generate();
    let keyset_bytes = [KeySetVersion::Version01.to_byte()]
        .into_iter()
        .chain([2u8; 32])
        .collect::<Vec<_>>();
    let keyset_id = Id::from_bytes(&keyset_bytes).unwrap();

    // Blind the primary key (slot 0)
    let primary_blinding = ecdh_kdf(&ephemeral_key, &primary_pubkey, keyset_id, 0).unwrap();
    let blinded_primary = blind_public_key(&primary_pubkey, &primary_blinding).unwrap();

    // Blind additional keys (slots 1-2)
    let add_blinding1 = ecdh_kdf(&ephemeral_key, &additional_pubkey1, keyset_id, 1).unwrap();
    let add_blinding2 = ecdh_kdf(&ephemeral_key, &additional_pubkey2, keyset_id, 2).unwrap();
    let blinded_add1 = blind_public_key(&additional_pubkey1, &add_blinding1).unwrap();
    let blinded_add2 = blind_public_key(&additional_pubkey2, &add_blinding2).unwrap();

    // Blind refund keys (slots 3-4)
    let refund_blinding1 = ecdh_kdf(&ephemeral_key, &refund_pubkey1, keyset_id, 3).unwrap();
    let refund_blinding2 = ecdh_kdf(&ephemeral_key, &refund_pubkey2, keyset_id, 4).unwrap();
    let blinded_refund1 = blind_public_key(&refund_pubkey1, &refund_blinding1).unwrap();
    let blinded_refund2 = blind_public_key(&refund_pubkey2, &refund_blinding2).unwrap();

    // Verify all keys are properly blinded (different from original)
    assert_ne!(primary_pubkey.to_string(), blinded_primary.to_string());
    assert_ne!(additional_pubkey1.to_string(), blinded_add1.to_string());
    assert_ne!(additional_pubkey2.to_string(), blinded_add2.to_string());
    assert_ne!(refund_pubkey1.to_string(), blinded_refund1.to_string());
    assert_ne!(refund_pubkey2.to_string(), blinded_refund2.to_string());

    // Test receiver-side key recovery
    // This simulates what a receiver would do with the ephemeral pubkey
    // They would derive the same blinding scalar for each key
    let ephemeral_pubkey = ephemeral_key.public_key();

    // Derive blinding scalar for the primary key on receiver side
    let receiver_primary_blinding =
        ecdh_kdf(&primary_key, &ephemeral_pubkey, keyset_id, 0).unwrap();

    // Verify that both sides derive the same blinding factor
    assert_eq!(
        primary_blinding.to_secret_bytes(),
        receiver_primary_blinding.to_secret_bytes()
    );

    // Similarly, test additional keys and refund keys recovery
    let receiver_add_blinding1 =
        ecdh_kdf(&additional_key1, &ephemeral_pubkey, keyset_id, 1).unwrap();
    assert_eq!(
        add_blinding1.to_secret_bytes(),
        receiver_add_blinding1.to_secret_bytes()
    );

    let receiver_add_blinding2 =
        ecdh_kdf(&additional_key2, &ephemeral_pubkey, keyset_id, 2).unwrap();
    assert_eq!(
        add_blinding2.to_secret_bytes(),
        receiver_add_blinding2.to_secret_bytes()
    );

    let receiver_refund_blinding1 =
        ecdh_kdf(&refund_key1, &ephemeral_pubkey, keyset_id, 3).unwrap();
    assert_eq!(
        refund_blinding1.to_secret_bytes(),
        receiver_refund_blinding1.to_secret_bytes()
    );

    let receiver_refund_blinding2 =
        ecdh_kdf(&refund_key2, &ephemeral_pubkey, keyset_id, 4).unwrap();
    assert_eq!(
        refund_blinding2.to_secret_bytes(),
        receiver_refund_blinding2.to_secret_bytes()
    );
}

#[test]
fn test_slot_numbers_are_consecutive() {
    let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

    // Create 3 different keys
    let key1 = SecretKey::generate().public_key();
    let key2 = SecretKey::generate().public_key();
    let key3 = SecretKey::generate().public_key();

    let ephemeral_sk = SecretKey::generate();

    let conditions = SpendingConditions::P2PKConditions {
        data: key1,
        conditions: Some(Conditions {
            pubkeys: Some(vec![key2, key3]),
            refund_keys: None,
            num_sigs: Some(1),
            sig_flag: SigFlag::SigInputs,
            locktime: None,
            num_sigs_refund: None,
        }),
    };

    let (_, blinded) =
        PreMintSecrets::apply_p2bk(conditions, keyset_id, Some(ephemeral_sk.clone())).unwrap();

    // Extract blinded keys
    let (blinded_key1, blinded_others) = match blinded {
        SpendingConditions::P2PKConditions { data, conditions } => {
            (data, conditions.unwrap().pubkeys.unwrap())
        }
        _ => panic!("Wrong type"),
    };

    // For each slot, try to derive and see if it matches
    // Slot 0 should match key1
    let r0 = ecdh_kdf(&ephemeral_sk, &key1, keyset_id, 0).unwrap();
    let test0 = blind_public_key(&key1, &r0).unwrap();
    assert_eq!(blinded_key1, test0, "Slot 0 should be key1");

    // Slot 1 should match key2
    let r1 = ecdh_kdf(&ephemeral_sk, &key2, keyset_id, 1).unwrap();
    let test1 = blind_public_key(&key2, &r1).unwrap();
    assert_eq!(blinded_others[0], test1, "Slot 1 should be key2");

    // Slot 2 should match key3 (FAILS with buggy code - it uses slot 3!)
    let r2 = ecdh_kdf(&ephemeral_sk, &key3, keyset_id, 2).unwrap();
    let test2 = blind_public_key(&key3, &r2).unwrap();
    assert_eq!(blinded_others[1], test2, "Slot 2 should be key3");
}
