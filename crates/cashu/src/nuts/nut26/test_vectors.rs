use std::str::FromStr;

use super::{blind_public_key, derive_signing_key_bip340, ecdh_kdf};
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut02::Id;

/// Tests for the NUT-26 test vectors
/// Based on: https://github.com/robwoodgate/nuts/blob/p2bk-silent/tests/XX-tests.md
#[test]
fn test_p2bk_test_vectors() {
    // Test inputs from the test vectors
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

    // Keyset ID from test vectors
    let keyset_id_hex = "009a1f293253e41e";
    let keyset_id = Id::from_str(keyset_id_hex).unwrap();

    // Expected shared secret from test vectors
    // let expected_shared_secret = "40d6ba4430a6dfa915bb441579b0f4dee032307434e9957a092bbca73151df8b";

    // Expected blinding scalars from test vectors
    let expected_blinding_scalars = [
        "41b5f15975f787bd5bd8d91753cbbe56d0d7aface851b1063e8011f68551862d", // r0
        "c4d68c79b8676841f767bcd53437af3f43d51b205f351d5cdfe5cb866ec41494", // r1
        "04ecf53095882f28965f267e46d2c555f15bcd74c3a84f42cf0de8ebfb712c7c", // r2
        "4163bc31b3087901b8b28249213b0ecc447cee3ea1f0c04e4dd5934e0c3f78ad", // r3
        "f5d6d20c399887f29bdda771660f87226e3a0d4ef36a90f40d3f717085957b60", // r4
        "f275404a115cd720ee099f5d6b7d5dc705d1c95ac6ae01c917031b64f7dccc72", // r5
        "39dffa9f0160bcda63920305fc12f88d824f5b654970dbd579c08367c12fcd78", // r6
        "3331338e87608c7f36265c9b52bb5ebeac1bb3e2220d2682370f4b7c09dccd4b", // r7
        "44947bd36c0200fb5d5d05187861364f6b666aac8ce37b368e27f01cea7cf147", // r8
        "cf4e69842833e0dab8a7302933d648fee98de80284af2d7ead71b420a8f0ebde", // r9
        "3638eae8a9889bbd96769637526010b34cd1e121805eaaaaa0602405529ca92f", // r10
    ];

    // Expected blinded public keys
    let expected_blinded_pubkeys = [
        "03f221b62aa21ee45982d14505de2b582716ae95c265168f586dc547f0ea8f135f", // P'0
        "0299692178029fe08c49e8123bb0e84d6e960b27f82c8aed43013526489d46c0d5", // P'1
        "03ae189850bda004f9723e17372c99ff9df9e29750d2147d40efb45ac8ab2cdd2c", // P'2
        "03109838d718fbe02e9458ffa423f25bae0388146542534f8e2a094de6f7b697fa", // P'3
        "0339d5ed7ea93292e60a4211b2daf20dff53f050835614643a43edccc35c8313db", // P'4
        "0237861efcd52fe959bce07c33b5607aeae0929749b8339f68ba4365f2fb5d2d8d", // P'5
        "026d5500988a62cde23096047db61e9fb5ef2fea5c521019e23862108ea4e14d72", // P'6
        "039024fd20b26e73143509537d7c18595cfd101da4b18bb86ddd30e944aac6ef1b", // P'7
        "03017ec4218ca2ed0fbe050e3f1a91221407bf8c896b803a891c3a52d162867ef8", // P'8
        "0380dc0d2c79249e47b5afb61b7d40e37b9b0370ec7c80b50c62111021b886ab31", // P'9
        "0261a8a32e718f5f27610a2b7c2069d6bab05d1ead7da21aa9dd2a3c758bdf6479", // P'10
    ];

    // Expected standard derived secret keys (p + r)
    let expected_std_secret_keys = [
        "eeedda054df845fbde4b8a579952fd9a240a2e9ad3c1dc791c4c6e51654698c9", // sk0
        "720e75259068268079da6e1579beee83dc58bd279b5ca893fddfc9547e82e5ef", // sk1
        "b224dddc6d88ed6718d1d7be8c5a0499448e4c62af187ab5acda4546db663f18", // sk2
        "ee9ba4dd8b0937403b25338966c24e0f97af6d2c8d60ebc12ba1efa8ec348b49", // sk3
        "a30ebab8119946311e5058b1ab96c66706bdaf562f921c2b2b396f3e95544cbb", // sk4
        "9fad28f5e95d955f707c509db1049d0b9e556b6202d58d0034fd1933079b9dcd", // sk5
        "e717e34ad9617b18e604b446419a37d0d581da5334e10748578cdfc2a124e014", // sk6
        "e0691c3a5f614abdb8990ddb98429e01ff4e32d00d7d51f514dba7d6e9d1dfe7", // sk7
        "f1cc647f4402bf39dfcfb658bde87592be98e99a7853a6a96bf44c77ca7203e3", // sk8
        "7c86523000349f193b19e169795d884382118a09c0d6b8b5cb6bb1eeb8afbd39", // sk9
        "e370d394818959fc18e9477797e74ff6a004600f6bced61d7e2c80603291bbcb", // sk10
    ];

    // Expected negated derived secret keys (-p + r)
    let expected_neg_secret_keys = [
        "947e08ad9df6c97ed96627d70e447f1238540da5ac2a25cf208614287592b4d2", // sk_neg0
        "179ea3cde066aa0374f50b94eeb06ffbf0a29c3273c4f1ea02196f2b8ecf01f8", // sk_neg1
        "57b50c84bd8770ea13ec753e014b861158d82b6d8780c40bb113eb1debb25b21", // sk_neg2
        "942bd385db07bac3363fd108dbb3cf87abf94c3765c935172fdb957ffc80a752", // sk_neg3
        "489ee9606197c9b4196af631208847df1b078e6107fa65812f731515a5a068c4", // sk_neg4
        "453d579e395c18e26b96ee1d25f61e83b29f4a6cdb3dd6563936bf0a17e7b9d6", // sk_neg5
        "8ca811f3295ffe9be11f51c5b68bb948e9cbb95e0d49509e5bc68599b170fc1d", // sk_neg6
        "85f94ae2af5fce40b3b3ab5b0d341f7a139811dae5e59b4b19154dadfa1dfbf0", // sk_neg7
        "975c9327940142bcdaea53d832d9f70ad2e2c8a550bbefff702df24edabe1fec", // sk_neg8
        "221680d85033229c36347ee8ee4f09bb965b6914993f020bcfa557c5c8fbd942", // sk_neg9
        "8901023cd187dd7f1403e4f70cd8d16eb44e3f1a44371f738266263742ddd7d4", // sk_neg10
    ];

    let mut fail = false;

    println!("=== Testing ECDH KDF Function ===");

    // Test 1: Verify ECDH KDF (check if we can derive the blinding scalars)
    for slot in 0..=10 {
        // Calculate the blinding scalar using our KDF
        let r_sender =
            ecdh_kdf(&ephemeral_secret_key, &receiver_public_key, keyset_id, slot).unwrap();
        let r_receiver =
            ecdh_kdf(&receiver_secret_key, &ephemeral_public_key, keyset_id, slot).unwrap();

        // Check if sender and receiver derive the same scalar
        if r_sender.to_string() != r_receiver.to_string() {
            fail = true;
            println!(
                "❌ ECDH KDF FAIL (slot {}): Sender and receiver derive different scalars",
                slot
            );
            println!("  Sender:   {}", r_sender);
            println!("  Receiver: {}", r_receiver);
        } else {
            println!(
                "✓ ECDH KDF (slot {}): Sender and receiver derive same scalar",
                slot
            );
        }

        // Check if our derived scalar matches the test vector
        let expected_scalar = expected_blinding_scalars[slot as usize];
        if r_sender.to_string() != expected_scalar {
            fail = true;
            println!(
                "❌ ECDH KDF FAIL (slot {}): Derived scalar doesn't match test vector",
                slot
            );
            println!("  Derived:  {}", r_sender);
            println!("  Expected: {}", expected_scalar);
        } else {
            println!(
                "✓ ECDH KDF (slot {}): Derived scalar matches test vector",
                slot
            );
        }
    }

    println!("\n=== Testing Blind Public Key Function ===");

    // Test 2: Verify blind_public_key (given the expected blinding scalar)
    for slot in 0..=10 {
        // Use the expected blinding scalar from test vectors
        let blinding_scalar =
            SecretKey::from_hex(expected_blinding_scalars[slot as usize]).unwrap();

        // Blind the public key
        let blinded_pubkey = blind_public_key(&receiver_public_key, &blinding_scalar).unwrap();

        // Check if it matches the expected blinded pubkey
        let expected_pubkey = expected_blinded_pubkeys[slot as usize];
        if blinded_pubkey.to_string() != expected_pubkey {
            fail = true;
            println!(
                "❌ BLIND PUBKEY FAIL (slot {}): Blinded pubkey doesn't match test vector",
                slot
            );
            println!("  Derived:  {}", blinded_pubkey);
            println!("  Expected: {}", expected_pubkey);
        } else {
            println!(
                "✓ BLIND PUBKEY (slot {}): Blinded pubkey matches test vector",
                slot
            );
        }
    }

    println!("\n=== Testing BIP340 Derivation Function ===");

    // Test 4: Verify derive_signing_key_bip340 (given the expected blinding scalar and blinded pubkey)
    for slot in 0..=10 {
        // Use the expected blinding scalar from test vectors
        let blinding_scalar =
            SecretKey::from_hex(expected_blinding_scalars[slot as usize]).unwrap();

        // Get the expected blinded pubkey
        let blinded_pubkey = PublicKey::from_hex(expected_blinded_pubkeys[slot as usize]).unwrap();

        // Derive the signing key
        let signing_key =
            derive_signing_key_bip340(&receiver_secret_key, &blinding_scalar, &blinded_pubkey)
                .unwrap();

        // Check if it matches one of the expected keys (std or neg)
        let is_std_key = signing_key.to_string() == expected_std_secret_keys[slot as usize];
        let is_neg_key = signing_key.to_string() == expected_neg_secret_keys[slot as usize];

        if !is_std_key && !is_neg_key {
            fail = true;
            println!(
                "❌ BIP340 DERIVATION FAIL (slot {}): Key doesn't match either std or neg",
                slot
            );
            println!("  Derived:     {}", signing_key);
            println!(
                "  Expected std: {}",
                expected_std_secret_keys[slot as usize]
            );
            println!(
                "  Expected neg: {}",
                expected_neg_secret_keys[slot as usize]
            );
        } else {
            println!(
                "✓ BIP340 DERIVATION (slot {}): Key matches {}",
                slot,
                if is_std_key { "standard" } else { "negated" }
            );
        }

        // Verify the public key's x-coordinate matches the expected blinded pubkey's x-coordinate
        let derived_pubkey = signing_key.public_key();
        if derived_pubkey.x_only_public_key() != blinded_pubkey.x_only_public_key() {
            fail = true;
            println!(
                "❌ BIP340 PUBKEY FAIL (slot {}): X-coordinate mismatch",
                slot
            );
            println!(
                "  Derived pubkey x: {:?}",
                derived_pubkey.x_only_public_key()
            );
            println!(
                "  Expected pubkey x: {:?}",
                blinded_pubkey.x_only_public_key()
            );
        } else {
            println!("✓ BIP340 PUBKEY (slot {}): X-coordinate matches", slot);
        }
    }

    assert_eq!(fail, false);
}
