#![no_main]

//! Fuzz NUT-28 ECDH symmetry and blinded-key recovery invariants.

use cashu::nuts::nut28::{blind_public_key, derive_signing_key_bip340, ecdh_kdf};
use cashu::SecretKey;
use cdk_fuzz::secret_key_from;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    sender: [u8; 32],
    receiver: [u8; 32],
    wrong_receiver: [u8; 32],
    slot: u8,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            sender: u.arbitrary()?,
            receiver: u.arbitrary()?,
            wrong_receiver: u.arbitrary()?,
            slot: u.arbitrary()?,
        })
    }
}

fn distinct_key(candidate: [u8; 32], key: &SecretKey) -> SecretKey {
    let mut candidate = secret_key_from(candidate);
    if candidate.public_key() == key.public_key() {
        candidate = secret_key_from([2; 32]);
    }
    if candidate.public_key() == key.public_key() {
        candidate = secret_key_from([3; 32]);
    }
    candidate
}

fuzz_target!(|input: Input| {
    let sender = secret_key_from(input.sender);
    let receiver = secret_key_from(input.receiver);
    let wrong_receiver = distinct_key(input.wrong_receiver, &receiver);
    let slot = input.slot % 11;

    let sender_blinding = ecdh_kdf(&sender, &receiver.public_key(), slot)
        .expect("canonical slots must derive a blinding scalar");
    let receiver_blinding = ecdh_kdf(&receiver, &sender.public_key(), slot)
        .expect("ECDH must derive from the receiver side");
    assert_eq!(
        sender_blinding.to_secret_bytes(),
        receiver_blinding.to_secret_bytes(),
        "sender and receiver must derive the same scalar"
    );
    assert_eq!(
        sender_blinding.to_secret_bytes(),
        ecdh_kdf(&sender, &receiver.public_key(), slot)
            .expect("repeated KDF must succeed")
            .to_secret_bytes(),
        "NUT-28 KDF must be deterministic"
    );

    let blinded = blind_public_key(&receiver.public_key(), &sender_blinding)
        .expect("valid points must blind");
    let signing_key = derive_signing_key_bip340(&receiver, &receiver_blinding, &blinded)
        .expect("receiver must recover the blinded signing key");
    assert_eq!(
        signing_key.public_key().x_only_public_key(),
        blinded.x_only_public_key(),
        "recovered BIP-340 key must match the blinded x-only key"
    );

    assert!(
        ecdh_kdf(&sender, &receiver.public_key(), 11 + input.slot % 245).is_err(),
        "non-canonical slots must be rejected"
    );
    assert!(
        derive_signing_key_bip340(&wrong_receiver, &receiver_blinding, &blinded).is_err(),
        "a different receiver key must not recover the blinded key"
    );

    if slot < 10 {
        let next = ecdh_kdf(&sender, &receiver.public_key(), slot + 1)
            .expect("next canonical slot must derive");
        assert_ne!(
            sender_blinding.to_secret_bytes(),
            next.to_secret_bytes(),
            "different canonical slots must derive different scalars"
        );
    }
});
