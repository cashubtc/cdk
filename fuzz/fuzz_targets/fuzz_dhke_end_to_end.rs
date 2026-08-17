#![no_main]

//! Fuzz the complete Cashu DHKE blind-sign-unblind-verify flow.

use std::collections::BTreeMap;

use cashu::dhke::{
    blind_message, construct_proofs, sign_message, unblind_message, verify_message,
};
use cashu::nuts::nut00::BlindSignature;
use cashu::nuts::nut01::Keys;
use cashu::secret::Secret;
use cashu::{Amount, Id, SecretKey};
use cdk_fuzz::arbitrary_ext::IdArb;
use cdk_fuzz::secret_key_from;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    message: Vec<u8>,
    mint_key: [u8; 32],
    blinding: [u8; 32],
    wrong_key: [u8; 32],
    amount: u64,
    keyset_id: IdArb,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let message_len = u.int_in_range(0..=128)?;
        Ok(Self {
            message: u.bytes(message_len)?.to_vec(),
            mint_key: u.arbitrary()?,
            blinding: u.arbitrary()?,
            wrong_key: u.arbitrary()?,
            amount: u.arbitrary()?,
            keyset_id: IdArb::arbitrary(u)?,
        })
    }
}

fn distinct_key(candidate: [u8; 32], mint_key: &SecretKey) -> SecretKey {
    let mut candidate = secret_key_from(candidate);
    if candidate.public_key() == mint_key.public_key() {
        candidate = secret_key_from([2; 32]);
    }
    if candidate.public_key() == mint_key.public_key() {
        candidate = secret_key_from([3; 32]);
    }
    candidate
}

fuzz_target!(|input: Input| {
    let amount = Amount::from(input.amount);
    let keyset_id: Id = input.keyset_id.into_inner();
    let mint_key = secret_key_from(input.mint_key);
    let wrong_key = distinct_key(input.wrong_key, &mint_key);
    let blinding = secret_key_from(input.blinding);

    let (blinded, returned_blinding) = blind_message(&input.message, Some(blinding.clone()))
        .expect("valid message and scalar must blind");
    assert_eq!(
        returned_blinding.to_secret_bytes(),
        blinding.to_secret_bytes()
    );
    let blinded_signature =
        sign_message(&mint_key, &blinded).expect("valid mint key must sign a blinded point");
    let unblinded = unblind_message(&blinded_signature, &blinding, &mint_key.public_key())
        .expect("valid blinded signature must unblind");
    assert!(
        verify_message(&mint_key, unblinded, &input.message).is_ok(),
        "complete DHKE flow must verify"
    );
    assert!(
        verify_message(&wrong_key, unblinded, &input.message).is_err(),
        "unblinded signature must reject a different mint key"
    );
    let mut wrong_message = input.message.clone();
    wrong_message.push(0xff);
    assert!(
        verify_message(&mint_key, unblinded, &wrong_message).is_err(),
        "unblinded signature must reject a different message"
    );

    let promise = BlindSignature {
        amount,
        keyset_id,
        c: blinded_signature,
        dleq: None,
    };
    let mut key_map = BTreeMap::new();
    key_map.insert(amount, mint_key.public_key());
    let keys = Keys::new(key_map);
    let secret = Secret::new(cashu::util::hex::encode(&input.message));
    let proofs = construct_proofs(
        vec![promise.clone()],
        vec![blinding],
        vec![secret],
        &keys,
    )
    .expect("matching promise inputs and amount key must construct a proof");
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].c, unblinded);

    assert!(
        construct_proofs(
            vec![promise],
            Vec::new(),
            vec![Secret::new("mismatched")],
            &keys,
        )
        .is_err(),
        "mismatched construction vectors must be rejected"
    );
});
