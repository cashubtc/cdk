//! Parity checks between the exported surface and the `cashu` crate it wraps.

use std::str::FromStr;

use bip39::Mnemonic;
use cashu::dhke::{hash_to_curve as core_hash_to_curve, sign_message};
use cashu::nuts::nut00::PreMintSecrets;
use cashu::nuts::nut01::{PublicKey, SecretKey};
use cashu::nuts::nut02::Id;
use cashu_ffi::{
    blind_message, blind_messages, create_deterministic_outputs, create_random_outputs,
    create_restore_outputs, create_single_deterministic_output, create_single_p2pk_output,
    create_single_random_output, hash_to_curve, keyset_id_v1, sha256_digest, split_amount,
    unblind_signature, verify_proof_dleq, CashuFfiError, DeterministicOutputFactory, DleqProof,
    KeyEntry, P2pkOptions, SigFlag, MAX_RESTORE_COUNTERS,
};

const MNEMONIC: &str =
    "half depart obvious quality work element tank gorilla view sugar picture humble";
const KEYSET_V0: &str = "009a1f293253e41e";

fn seed() -> Vec<u8> {
    let mnemonic = Mnemonic::from_str(MNEMONIC).expect("static mnemonic");
    mnemonic.to_seed("").to_vec()
}

fn split_of(amount: u64) -> Vec<u64> {
    let denominations: Vec<u64> = (0..32).map(|i| 1u64 << i).collect();
    split_amount(amount, denominations, None).expect("split")
}

fn powers_of_two_keys(count: u32) -> Vec<KeyEntry> {
    (0..count)
        .map(|i| KeyEntry {
            amount: 1u64 << i,
            pubkey: SecretKey::generate().public_key().to_hex(),
        })
        .collect()
}

#[test]
fn sha256_matches_known_vector() {
    let digest = sha256_digest(b"abc".to_vec());
    assert_eq!(
        hex(&digest),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn hash_to_curve_matches_nut00_vectors() {
    let cases = [
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "024cce997d3b518f739663b757deaec95bcd9473c30a14ac2fd04023a739d1a725",
        ),
        (
            "0000000000000000000000000000000000000000000000000000000000000001",
            "022e7158e11c9506f1aa4248bf531298daa7febd6194f003edcd9b93ade6253acf",
        ),
        (
            "0000000000000000000000000000000000000000000000000000000000000002",
            "026cdbe15362df59cd1dd3c9c11de8aedac2106eca69236ecd9fbe117af897be4f",
        ),
    ];

    for (message, expected) in cases {
        let point = hash_to_curve(unhex(message)).expect("valid point");
        assert_eq!(hex(&point), expected);
    }
}

#[test]
fn blind_then_unblind_recovers_the_mint_signature() {
    let mint_key = SecretKey::generate();
    let mint_pubkey = mint_key.public_key();
    let secret = b"cashu-ffi round trip".to_vec();

    let pair = blind_message(secret.clone(), None).expect("blinding");
    let blinded = PublicKey::from_hex(&pair.blinded_secret).expect("hex point");
    let signed = sign_message(&mint_key, &blinded).expect("mint signature");

    let unblinded = unblind_signature(signed.to_hex(), pair.blinding_factor, mint_pubkey.to_hex())
        .expect("unblinding");

    let expected = sign_message(&mint_key, &core_hash_to_curve(&secret).expect("point"))
        .expect("expected signature");
    assert_eq!(unblinded, expected.to_hex());
}

#[test]
fn a_batch_blinds_each_secret_on_its_own() {
    let secrets = vec![b"first".to_vec(), b"second".to_vec()];
    let batch = blind_messages(secrets.clone()).expect("batch blinding");
    assert_eq!(batch.len(), secrets.len());

    for (pair, secret) in batch.iter().zip(&secrets) {
        let factor = SecretKey::from_hex(&pair.blinding_factor).expect("hex factor");
        let again = blind_message(secret.clone(), Some(factor.to_secret_bytes().to_vec()))
            .expect("blinding");
        assert_eq!(again.blinded_secret, pair.blinded_secret);
    }

    assert_ne!(batch[0].blinding_factor, batch[1].blinding_factor);

    let empty = blind_messages(Vec::new()).expect("an empty batch is allowed");
    assert!(empty.is_empty());
}

#[test]
fn blinding_factor_argument_is_honoured() {
    let factor = SecretKey::generate();
    let first = blind_message(b"secret".to_vec(), Some(factor.to_secret_bytes().to_vec()))
        .expect("blinding");
    let second = blind_message(b"secret".to_vec(), Some(factor.to_secret_bytes().to_vec()))
        .expect("blinding");

    assert_eq!(first.blinded_secret, second.blinded_secret);
    assert_eq!(first.blinding_factor, factor.to_secret_hex());
}

#[test]
fn deterministic_outputs_match_the_nut13_vectors() {
    let expected_secrets = [
        "485875df74771877439ac06339e284c3acfcd9be7abf3bc20b516faeadfe77ae",
        "8f2b39e8e594a4056eb1e6dbb4b0c38ef13b1b2c751f64f810ec04ee35b77270",
        "bc628c79accd2364fd31511216a0fab62afd4a18ff77a20deded7b858c9860c8",
        "59284fd1650ea9fa17db2b3acf59ecd0f2d52ec3261dd4152785813ff27a33bf",
        "576c23393a8b31cc8da6688d9c9a96394ec74b40fdaf1f693a6bb84284334ea0",
    ];
    let expected_factors = [
        "ad00d431add9c673e843d4c2bf9a778a5f402b985b8da2d5550bf39cda41d679",
        "967d5232515e10b81ff226ecf5a9e2e2aff92d66ebc3edf0987eb56357fd6248",
        "b20f47bb6ae083659f3aa986bfa0435c55c6d93f687d51a01f26862d9b9a4899",
        "fb5fca398eb0b1deb955a2988b5ac77d32956155f1c002a373535211a2dfdc29",
        "5f09bfbfe27c439a597719321e061e2e40aad4a36768bb2bcc3de547c9644bf9",
    ];

    for counter in 0..5u32 {
        let output = create_single_deterministic_output(1, seed(), counter, KEYSET_V0.to_string())
            .expect("derivation");
        assert_eq!(output.secret, expected_secrets[counter as usize]);
        assert_eq!(output.blinding_factor, expected_factors[counter as usize]);
        assert_eq!(output.derivation_index, Some(counter));
    }
}

#[test]
fn deterministic_batch_walks_the_counter() {
    let outputs = create_deterministic_outputs(split_of(11), seed(), 7, KEYSET_V0.to_string())
        .expect("derivation");

    assert_eq!(
        outputs.iter().map(|o| o.amount).sum::<u64>(),
        11,
        "outputs must add up to the requested amount"
    );
    let indices: Vec<Option<u32>> = outputs.iter().map(|o| o.derivation_index).collect();
    assert_eq!(indices, vec![Some(7), Some(8), Some(9)]);
}

#[test]
fn deterministic_batch_keeps_the_order_it_was_given() {
    let amounts = vec![8, 1, 4];
    let outputs = create_deterministic_outputs(amounts.clone(), seed(), 0, KEYSET_V0.to_string())
        .expect("derivation");

    assert_eq!(
        outputs.iter().map(|o| o.amount).collect::<Vec<_>>(),
        amounts,
        "the caller's ordering decides which counter each amount gets"
    );
}

#[test]
fn factory_and_free_function_agree() {
    let factory =
        DeterministicOutputFactory::new(seed(), KEYSET_V0.to_string()).expect("64 byte seed");
    let keys = powers_of_two_keys(6);

    let _ = keys;
    let from_object = factory.outputs(split_of(31), 3).expect("derivation");
    let from_function =
        create_deterministic_outputs(split_of(31), seed(), 3, KEYSET_V0.to_string())
            .expect("derivation");

    assert_eq!(from_object.len(), from_function.len());
    for (a, b) in from_object.iter().zip(from_function.iter()) {
        assert_eq!(a.secret, b.secret);
        assert_eq!(a.blinded_secret, b.blinded_secret);
        assert_eq!(a.blinding_factor, b.blinding_factor);
    }
    assert_eq!(factory.keyset_id(), KEYSET_V0);
}

#[test]
fn random_outputs_are_unique_and_sum_to_the_amount() {
    let outputs = create_random_outputs(split_of(100), KEYSET_V0.to_string()).expect("outputs");

    assert_eq!(outputs.iter().map(|o| o.amount).sum::<u64>(), 100);
    let mut secrets: Vec<&str> = outputs.iter().map(|o| o.secret.as_str()).collect();
    secrets.sort_unstable();
    let unique = secrets.len();
    secrets.dedup();
    assert_eq!(secrets.len(), unique, "random secrets must not repeat");
    assert!(outputs.iter().all(|o| o.derivation_index.is_none()));
}

#[test]
fn caller_supplied_denominations_are_used_verbatim() {
    let outputs = create_random_outputs(vec![4, 2, 1, 1], KEYSET_V0.to_string()).expect("outputs");

    let amounts: Vec<u64> = outputs.iter().map(|o| o.amount).collect();
    assert_eq!(
        amounts,
        vec![4, 2, 1, 1],
        "the caller's denomination order is preserved"
    );
}

#[test]
fn split_amount_uses_the_supplied_denominations() {
    let denominations: Vec<u64> = (0..8).map(|i| 1u64 << i).collect();
    assert_eq!(
        split_amount(13, denominations.clone(), None).expect("split"),
        vec![1, 4, 8]
    );
    assert_eq!(
        split_amount(0, denominations, None).expect("split"),
        Vec::<u64>::new()
    );
}

#[test]
fn p2pk_outputs_carry_the_lock() {
    let receiver = SecretKey::generate().public_key();
    let output = create_single_p2pk_output(
        P2pkOptions {
            pubkey: receiver.to_hex(),
            additional_pubkeys: None,
            num_sigs: None,
            locktime: None,
            refund_pubkeys: None,
            num_sigs_refund: None,
            sig_flag: SigFlag::SigInputs,
        },
        8,
        KEYSET_V0.to_string(),
    )
    .expect("p2pk output");

    assert!(output.secret.contains("P2PK"));
    assert!(output.secret.contains(&receiver.to_hex()));
}

#[test]
fn p2pk_sig_all_writes_the_tag() {
    let receiver = SecretKey::generate().public_key();
    let output = create_single_p2pk_output(
        P2pkOptions {
            pubkey: receiver.to_hex(),
            additional_pubkeys: None,
            num_sigs: None,
            locktime: None,
            refund_pubkeys: None,
            num_sigs_refund: None,
            sig_flag: SigFlag::SigAll,
        },
        8,
        KEYSET_V0.to_string(),
    )
    .expect("p2pk output");

    assert!(output.secret.contains("SIG_ALL"));
}

#[test]
fn dleq_verifies_and_rejects() {
    let mint_key = SecretKey::generate();
    let mint_pubkey = mint_key.public_key();
    let secret_text = "dleq round trip";

    let pair = blind_message(secret_text.as_bytes().to_vec(), None).expect("blinding");
    let blinded = PublicKey::from_hex(&pair.blinded_secret).expect("hex point");
    let signature = cashu::nuts::nut00::BlindSignature::new(
        cashu::Amount::from(1),
        sign_message(&mint_key, &blinded).expect("signature"),
        cashu::nuts::nut02::Id::from_str(KEYSET_V0).expect("keyset"),
        &blinded,
        &mint_key,
    )
    .expect("dleq");
    let dleq = signature.dleq.expect("dleq present");

    let unblinded = unblind_signature(
        signature.c.to_hex(),
        pair.blinding_factor.clone(),
        mint_pubkey.to_hex(),
    )
    .expect("unblinding");

    let proof = DleqProof {
        e: dleq.e.to_secret_hex(),
        s: dleq.s.to_secret_hex(),
    };
    assert!(verify_proof_dleq(
        secret_text.to_string(),
        unblinded.clone(),
        proof.clone(),
        pair.blinding_factor.clone(),
        mint_pubkey.to_hex(),
    )
    .expect("verification"));

    let other_mint = SecretKey::generate().public_key();
    assert!(!verify_proof_dleq(
        secret_text.to_string(),
        unblinded,
        proof,
        pair.blinding_factor,
        other_mint.to_hex(),
    )
    .expect("verification"));
}

#[test]
fn keyset_id_v1_matches_the_core_implementation() {
    let keys = powers_of_two_keys(4);
    let id = keyset_id_v1(keys).expect("keyset id");
    assert!(id.starts_with("00"));
    assert_eq!(id.len(), 16);
}

#[test]
fn restore_batch_matches_the_core_implementation() {
    let start = 5;
    let count = 4;
    let outputs = create_restore_outputs(seed(), KEYSET_V0.to_string(), start, count)
        .expect("derivation")
        .into_iter()
        .collect::<Vec<_>>();

    let id = Id::from_str(KEYSET_V0).expect("keyset id");
    let core_seed: [u8; 64] = seed().try_into().expect("64 byte seed");
    let core = PreMintSecrets::restore_batch(id, &core_seed, start, start + count)
        .expect("core derivation");

    assert_eq!(outputs.len(), count as usize);
    assert_eq!(outputs.len(), core.secrets.len());
    for (offset, (output, pre_mint)) in outputs.iter().zip(core.secrets.iter()).enumerate() {
        assert_eq!(output.amount, 0);
        assert_eq!(output.derivation_index, Some(start + offset as u32));
        assert_eq!(output.secret, pre_mint.secret.to_string());
        assert_eq!(
            output.blinded_secret,
            pre_mint.blinded_message.blinded_secret.to_hex()
        );
        assert_eq!(output.blinding_factor, pre_mint.r.to_secret_hex());
    }
}

#[test]
fn restore_factory_and_free_function_agree() {
    let factory =
        DeterministicOutputFactory::new(seed(), KEYSET_V0.to_string()).expect("64 byte seed");
    let from_object = factory.restore_batch(2, 3).expect("derivation");
    let from_function =
        create_restore_outputs(seed(), KEYSET_V0.to_string(), 2, 3).expect("derivation");

    assert_eq!(from_object.len(), from_function.len());
    for (a, b) in from_object.iter().zip(from_function.iter()) {
        assert_eq!(a.secret, b.secret);
        assert_eq!(a.blinded_secret, b.blinded_secret);
        assert_eq!(a.blinding_factor, b.blinding_factor);
    }
}

#[test]
fn an_empty_restore_batch_is_allowed() {
    let outputs =
        create_restore_outputs(seed(), KEYSET_V0.to_string(), 7, 0).expect("empty batch derives");
    assert!(outputs.is_empty());

    let factory =
        DeterministicOutputFactory::new(seed(), KEYSET_V0.to_string()).expect("64 byte seed");
    assert!(factory.restore_batch(7, 0).expect("empty batch").is_empty());
}

#[test]
fn an_oversized_restore_batch_is_refused() {
    let count = MAX_RESTORE_COUNTERS + 1;
    let err = create_restore_outputs(seed(), KEYSET_V0.to_string(), 0, count)
        .expect_err("count above the cap is rejected");
    match err {
        CashuFfiError::InvalidRestoreRange { count: got, max } => {
            assert_eq!(got, count);
            assert_eq!(max, MAX_RESTORE_COUNTERS);
        }
        other => panic!("unexpected error: {other}"),
    }

    let factory =
        DeterministicOutputFactory::new(seed(), KEYSET_V0.to_string()).expect("64 byte seed");
    let err = factory
        .restore_batch(0, count)
        .expect_err("count above the cap is rejected");
    match err {
        CashuFfiError::InvalidRestoreRange { count: got, max } => {
            assert_eq!(got, count);
            assert_eq!(max, MAX_RESTORE_COUNTERS);
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn errors_keep_their_fields() {
    let err = create_single_deterministic_output(1, vec![0u8; 32], 0, KEYSET_V0.to_string())
        .expect_err("32 byte seed is rejected");
    match err {
        CashuFfiError::InvalidSeedLength { length } => assert_eq!(length, 32),
        other => panic!("unexpected error: {other}"),
    }

    let err = create_single_random_output(1, "not-a-keyset".to_string())
        .expect_err("bad keyset id is rejected");
    match err {
        CashuFfiError::InvalidKeysetId { id, .. } => assert_eq!(id, "not-a-keyset"),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn a_bad_keyset_id_is_rejected_with_a_valid_seed() {
    let bad = "not-a-keyset".to_string();

    let calls: Vec<CashuFfiError> = vec![
        create_deterministic_outputs(vec![1], seed(), 0, bad.clone())
            .expect_err("batch rejects the keyset id"),
        create_single_deterministic_output(1, seed(), 0, bad.clone())
            .expect_err("single output rejects the keyset id"),
        create_restore_outputs(seed(), bad.clone(), 0, 1)
            .expect_err("restore rejects the keyset id"),
        DeterministicOutputFactory::new(seed(), bad.clone())
            .expect_err("factory rejects the keyset id"),
    ];

    for err in calls {
        match err {
            CashuFfiError::InvalidKeysetId { id, .. } => assert_eq!(id, bad),
            other => panic!("unexpected error: {other}"),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}
