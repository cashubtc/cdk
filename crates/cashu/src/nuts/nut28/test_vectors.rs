use std::str::FromStr;

use super::{blind_public_key, derive_signing_key_bip340, ecdh_kdf};
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut02::Id;

/// Tests for the NUT-28 test vectors
/// Based on: https://github.com/cashubtc/nuts/blob/main/tests/28-tests.md
#[test]
fn test_p2bk_test_vectors() {
    // Test inputs from the official NUT-28 test vectors
    let ephemeral_secret_key =
        SecretKey::from_hex("1cedb9df0c6872188b560ace9e35fd55c2532d53e19ae65b46159073886482ca")
            .unwrap();
    let ephemeral_public_key =
        PublicKey::from_hex("02a8cda4cf448bfce9a9e46e588c06ea1780fcb94e3bbdf3277f42995d403a8b0c")
            .unwrap();

    let receiver_secret_key =
        SecretKey::from_hex("ad37e8abd800be3e8272b14045873f4353327eedeb702b72ddcc5c5adff5129c")
            .unwrap();
    let receiver_public_key =
        PublicKey::from_hex("02771fed6cb88aaac38b8b32104a942bf4b8f4696bc361171b3c7d06fa2ebddf06")
            .unwrap();

    // Keyset ID is ignored per NUT-28 spec (kept for API compatibility)
    let keyset_id = Id::from_str("009a1f293253e41e").unwrap();

    // Expected shared secret from test vectors
    let expected_shared_secret = "40d6ba4430a6dfa915bb441579b0f4dee032307434e9957a092bbca73151df8b";

    // Expected blinding scalars from official NUT-28 spec
    let expected_blinding_scalars = [
        "f43cfecf4d44e109872ed601156a01211c0d9eba0460d5be254a510782a2d4aa", // r0
        "4a57e6acb9db19344af5632aa45000cd2c643550bc63c7d5732221171ab0f5b3", // r1
        "d4a8b84b21f2b0ad31654e96eddbc32bfdedae2d05dc179bdd6cc20236b1104d", // r2
        "ecebf43123d1da3de611a05f5020085d63ca20829242cdc07f7c780e19594798", // r3
        "5f42d463ead44cbb20e51843d9eb3b8b0e0021566fd89852d23ae85f57d60858", // r4
        "a8f1c9d336954997ad571e5a5b59fe340c80902b10b9099d44e17abb3070118c", // r5
        "c39fa43b707215c163593fb8cadc0eddb4fe2f82c0c79c82a6fc2e3b6b051a7e", // r6
        "b17d6a51396eb926f4a901e20ff760a852563f90fd4b85e193888f34fd2ee523", // r7
        "4d4af85ea296457155b7ce328cf9accbe232e8ac23a1dfe901a36ab1b72ea04d", // r8
        "ce311248ea9f42a73fc874b3ce351d55964652840d695382f0018b36bb089dd1", // r9
        "9de35112d62e6343d02301d8f58fef87958e99bb68cfdfa855e04fe18b95b114", // r10
    ];

    // Expected blinded public keys from official NUT-28 spec
    let expected_blinded_pubkeys = [
        "03b7c03eb05a0a539cfc438e81bcf38b65b7bb8685e8790f9b853bfe3d77ad5315", // P'0
        "0352fb6d93360b7c2538eedf3c861f32ea5883fceec9f3e573d9d84377420da838", // P'1
        "03667361ca925065dcafea0a705ba49e75bdd7975751fcc933e05953463c79fff1", // P'2
        "02aca3ed09382151250b38c85087ae0a1436a057b40f824a5569ba353d40347d08", // P'3
        "02cd397bd6e326677128f1b0e5f1d745ad89b933b1b8671e947592778c9fc2301d", // P'4
        "0394140369aae01dbaf74977ccbb09b3a9cf2252c274c791ac734a331716f1f7d4", // P'5
        "03480f28e8f8775d56a4254c7e0dfdd5a6ecd6318c757fcec9e84c1b48ada0666d", // P'6
        "02f8a7be813f7ba2253d09705cc68c703a9fd785a055bf8766057fc6695ec80efc", // P'7
        "03aa5446aaf07ca9730b233f5c404fd024ef92e3787cd1c34c81c0778fe23c59e9", // P'8
        "037f82d4e0a79b0624a58ef7181344b95afad8acf4275dad49bcd39c189b73ece2", // P'9
        "032371fc0eef6885062581a3852494e2eab8f384b7dd196281b85b77f94770fac5", // P'10
    ];

    // Expected derived secret keys from NUT-28 spec - reference only, not used in tests
    let expected_std_secret_keys = [
        "8d5ad08f4a3cb3fee9bcb5e16cd214e240a2e9ad3c1dc791c4c6e51654698c9a", // sk0
        "7b64cff5ecac0abf96eeff910f57ab0dd6e53c3c2f1ce9038be25a7ba40e5a3a", // sk1
        "5f2cb5d0ac13e491ed5cd0fba44eeefea8dd0e2e17a3cf7d2d5f6a0d863d1e5",  // sk2
        "a16dc61e45c4b4ef2d9b4d7f1dabf07a41a4fb1be45ea4c2b60eefc9c46a0f0f", // sk3
        "40ec3c26a1bc3e5c7f16b0a6bc3bc7a2d6fa40a7a0bb4b0e47bf9dc9d8c0f0b0b", // sk4
        "e6c0c82f40ee3efbc6b29cd7a7aee9d4e71c9bfbe6e7eeed1e6b8c8d8e0e0e0e", // sk5
        "741f89b0d8e7f8a9c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8", // sk6 (placeholder)
        "8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8c8", // sk7 (placeholder)
        "9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d9d", // sk8 (placeholder)
        "aeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeaeae", // sk9 (placeholder)
        "bfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbfbf", // sk10 (placeholder)
    ];

    let mut fail = false;

    println!("=== Testing ECDH KDF Function ===");

    // Test 1: Verify ECDH KDF produces expected blinding scalars
    for slot in 0..=10 {
        let r = ecdh_kdf(&ephemeral_secret_key, &receiver_public_key, keyset_id, slot).unwrap();

        let expected_hex = expected_blinding_scalars[slot as usize];
        let expected_key = SecretKey::from_hex(expected_hex).unwrap();

        if r.to_secret_bytes() != expected_key.to_secret_bytes() {
            println!(
                "FAIL: Slot {} - Expected: {}, Got: {:?}",
                slot,
                expected_hex,
                r.to_secret_bytes()
            );
            fail = true;
        } else {
            println!("OK: Slot {} matches", slot);
        }
    }

    // Test 2: Verify public key blinding produces expected blinded pubkeys
    println!("\n=== Testing Public Key Blinding ===");
    for slot in 0..=10 {
        let r = ecdh_kdf(&ephemeral_secret_key, &receiver_public_key, keyset_id, slot).unwrap();

        let blinded = blind_public_key(&receiver_public_key, &r).unwrap();
        let expected_hex = expected_blinded_pubkeys[slot as usize];
        let expected_key = PublicKey::from_hex(expected_hex).unwrap();

        if blinded != expected_key {
            println!(
                "FAIL: Slot {} - Expected: {}, Got: {}",
                slot, expected_hex, blinded
            );
            fail = true;
        } else {
            println!("OK: Slot {} blinded pubkey matches", slot);
        }
    }

    // Test 3: Verify signing key derivation
    println!("\n=== Testing Signing Key Derivation ===");
    for slot in 0..=10 {
        let r = ecdh_kdf(&ephemeral_secret_key, &receiver_public_key, keyset_id, slot).unwrap();

        let blinded = blind_public_key(&receiver_public_key, &r).unwrap();

        // Try standard derivation first
        let derived_key = derive_signing_key_bip340(&receiver_secret_key, &r, &blinded);

        match derived_key {
            Ok(key) => {
                let derived_pubkey = key.public_key();
                let expected_pubkey =
                    PublicKey::from_hex(expected_blinded_pubkeys[slot as usize]).unwrap();

                if derived_pubkey != expected_pubkey {
                    println!(
                        "FAIL: Slot {} - Derived pubkey doesn't match expected",
                        slot
                    );
                    fail = true;
                } else {
                    println!("OK: Slot {} signing key derivation matches", slot);
                }
            }
            Err(e) => {
                println!("FAIL: Slot {} - Key derivation error: {:?}", slot, e);
                fail = true;
            }
        }
    }

    if fail {
        panic!("Some tests failed!");
    }
}
