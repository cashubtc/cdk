#![no_main]

//! Fuzz NUT-20 and NUT-29 quote-signature commitments.

use cashu::nuts::nut04::MintRequest;
use cashu::nuts::nut29::BatchMintRequest;
use cashu::{Amount, BlindedMessage, SecretKey};
use cdk_fuzz::arbitrary_ext::{BlindedMessageArb, IdArb};
use cdk_fuzz::{pubkey_from, secret_key_from};
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    signing_key: [u8; 32],
    wrong_key: [u8; 32],
    quote: String,
    outputs: Vec<BlindedMessageArb>,
    replacement_key: [u8; 32],
    replacement_id: IdArb,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let output_count = u.int_in_range(0..=5)?;
        Ok(Self {
            signing_key: u.arbitrary()?,
            wrong_key: u.arbitrary()?,
            quote: String::arbitrary(u)?.chars().take(64).collect(),
            outputs: (0..output_count)
                .map(|_| BlindedMessageArb::arbitrary(u))
                .collect::<Result<_, _>>()?,
            replacement_key: u.arbitrary()?,
            replacement_id: IdArb::arbitrary(u)?,
        })
    }
}

fn distinct_key(candidate: [u8; 32], signing_key: &SecretKey) -> SecretKey {
    let mut candidate = secret_key_from(candidate);
    if candidate.public_key() == signing_key.public_key() {
        candidate = secret_key_from([2; 32]);
    }
    if candidate.public_key() == signing_key.public_key() {
        candidate = secret_key_from([3; 32]);
    }
    candidate
}

fn replacement_output(input: &Input, original: Option<&BlindedMessage>) -> BlindedMessage {
    let mut blinded_secret = pubkey_from(input.replacement_key);
    if original.is_some_and(|output| output.blinded_secret == blinded_secret) {
        blinded_secret = secret_key_from([4; 32]).public_key();
    }
    BlindedMessage::new(
        original
            .map(|output| Amount::from(output.amount.to_u64().wrapping_add(1)))
            .unwrap_or(Amount::ONE),
        input.replacement_id.clone().into_inner(),
        blinded_secret,
    )
}

fuzz_target!(|input: Input| {
    let signing_key = secret_key_from(input.signing_key);
    let signing_pubkey = signing_key.public_key();
    let wrong_key = distinct_key(input.wrong_key, &signing_key);
    let outputs = input
        .outputs
        .iter()
        .cloned()
        .map(BlindedMessageArb::into_inner)
        .collect::<Vec<_>>();

    let mut request = MintRequest {
        quote: input.quote.clone(),
        outputs: outputs.clone(),
        signature: None,
    };
    let message = request.msg_to_sign();
    assert_eq!(message, request.msg_to_sign(), "signing message must be deterministic");
    request
        .sign(&signing_key)
        .expect("valid key must sign a mint request");
    assert!(
        request.verify_signature(signing_pubkey).is_ok(),
        "fresh NUT-20 signature must verify"
    );
    assert!(
        request.verify_signature(wrong_key.public_key()).is_err(),
        "NUT-20 signature must reject a different key"
    );

    let mut changed_quote = request.clone();
    changed_quote.quote.push('#');
    assert!(
        changed_quote.verify_signature(signing_pubkey).is_err(),
        "signature must commit to the quote id"
    );

    let mut changed_outputs = request.clone();
    match changed_outputs.outputs.first_mut() {
        Some(first) => *first = replacement_output(&input, Some(first)),
        None => changed_outputs.outputs.push(replacement_output(&input, None)),
    }
    assert!(
        changed_outputs.verify_signature(signing_pubkey).is_err(),
        "signature must commit to every output"
    );

    let mut malformed_signature = request.clone();
    malformed_signature.signature = Some("00".to_string());
    assert!(malformed_signature.verify_signature(signing_pubkey).is_err());

    let mut legacy = MintRequest {
        quote: input.quote.clone(),
        outputs: outputs.clone(),
        signature: None,
    };
    legacy
        .sign_legacy(signing_key.clone())
        .expect("valid key must create a legacy signature");
    assert!(legacy.verify_signature(signing_pubkey).is_ok());

    let batch = BatchMintRequest {
        quotes: vec![input.quote.clone()],
        quote_amounts: None,
        outputs,
        signatures: None,
    };
    let signature = batch
        .sign_quote(&input.quote, &signing_key)
        .expect("valid key must sign a batch quote");
    assert!(
        batch
            .verify_quote_signature(&input.quote, &signature, &signing_pubkey)
            .is_ok(),
        "fresh NUT-29 signature must verify"
    );
    assert!(
        batch
            .verify_quote_signature(
                &format!("{}#", input.quote),
                &signature,
                &signing_pubkey,
            )
            .is_err(),
        "batch signature must commit to its quote id"
    );
});
