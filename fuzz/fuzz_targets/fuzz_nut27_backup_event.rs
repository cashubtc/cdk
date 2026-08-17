#![no_main]

//! Fuzz NUT-27 key derivation and encrypted mint-backup events.

use cashu::nuts::nut27::{
    backup_filter_params, create_backup_event, decrypt_backup_event, derive_nostr_keys, MintBackup,
};
use cdk_fuzz::arbitrary_ext::MintUrlArb;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    seed: [u8; 64],
    wrong_seed: [u8; 64],
    mints: Vec<MintUrlArb>,
    timestamp: u64,
    client: Option<String>,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let mint_count = u.int_in_range(0..=4)?;
        let mut mints = Vec::with_capacity(mint_count);
        for _ in 0..mint_count {
            mints.push(MintUrlArb::arbitrary(u)?);
        }

        let client = if bool::arbitrary(u)? {
            Some(String::arbitrary(u)?.chars().take(32).collect())
        } else {
            None
        };

        Ok(Self {
            seed: u.arbitrary()?,
            wrong_seed: u.arbitrary()?,
            mints,
            timestamp: u.arbitrary()?,
            client,
        })
    }
}

fuzz_target!(|input: Input| {
    let keys = derive_nostr_keys(&input.seed).expect("every seed must derive valid Nostr keys");
    let keys_again =
        derive_nostr_keys(&input.seed).expect("the same seed must derive valid Nostr keys");
    assert_eq!(
        keys.public_key(),
        keys_again.public_key(),
        "NUT-27 key derivation must be deterministic"
    );

    let mints = input
        .mints
        .iter()
        .cloned()
        .map(MintUrlArb::into_inner)
        .collect();
    let backup = MintBackup::with_timestamp(mints, input.timestamp);
    let serialized = serde_json::to_vec(&backup).expect("backup must serialize");
    let reparsed: MintBackup =
        serde_json::from_slice(&serialized).expect("serialized backup must deserialize");
    assert_eq!(reparsed, backup, "backup JSON round-trip mismatch");

    let event = create_backup_event(&keys, &backup, input.client.as_deref())
        .expect("valid backup must produce a signed event");
    assert_eq!(
        decrypt_backup_event(&keys, &event).expect("created event must decrypt"),
        backup,
        "decrypted backup mismatch"
    );

    let (kind, author, identifier) = backup_filter_params(&keys);
    assert_eq!(event.kind, kind, "filter and event kinds must agree");
    assert_eq!(event.pubkey, author, "filter and event authors must agree");
    assert_eq!(identifier, "mint-list");

    let mut wrong_seed = input.wrong_seed;
    if wrong_seed == input.seed {
        wrong_seed[0] ^= 1;
    }
    let wrong_keys =
        derive_nostr_keys(&wrong_seed).expect("every alternate seed must derive valid Nostr keys");
    assert!(
        decrypt_backup_event(&wrong_keys, &event).is_err(),
        "an unrelated key must not decrypt the backup"
    );

    let mut corrupted_content = event.clone();
    corrupted_content.content.push('x');
    assert!(
        decrypt_backup_event(&keys, &corrupted_content).is_err(),
        "corrupted ciphertext must be rejected"
    );

    let mut missing_tags = event;
    missing_tags.tags = Default::default();
    assert!(
        decrypt_backup_event(&keys, &missing_tags).is_err(),
        "an event without the mint-list identifier must be rejected"
    );
});
