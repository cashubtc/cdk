#![no_main]

//! Fuzz NUT-13 deterministic secret, blinding, and restore invariants.

use cashu::amount::{FeeAndAmounts, SplitTarget};
use cashu::dhke::blind_message;
use cashu::nuts::nut00::PreMintSecrets;
use cashu::secret::Secret;
use cashu::{Amount, SecretKey};
use cdk_fuzz::arbitrary_ext::IdArb;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    seed: [u8; 64],
    keyset_id: IdArb,
    counter: u32,
    amount: u8,
    batch_len: u8,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            seed: u.arbitrary()?,
            keyset_id: IdArb::arbitrary(u)?,
            counter: u.arbitrary()?,
            amount: u.arbitrary()?,
            batch_len: u.arbitrary()?,
        })
    }
}

fuzz_target!(|input: Input| {
    const MAX_COUNTER: u32 = (1 << 31) - 16;

    let keyset_id = input.keyset_id.into_inner();
    let counter = input.counter % MAX_COUNTER;
    let batch_len = u32::from(input.batch_len % 9);

    let secret = Secret::from_seed(&input.seed, keyset_id, counter)
        .expect("bounded counter must derive a deterministic secret");
    let secret_again = Secret::from_seed(&input.seed, keyset_id, counter)
        .expect("repeated secret derivation must succeed");
    assert_eq!(secret, secret_again, "secret derivation must be deterministic");

    let blinding = SecretKey::from_seed(&input.seed, keyset_id, counter)
        .expect("bounded counter must derive a blinding factor");
    let blinding_again = SecretKey::from_seed(&input.seed, keyset_id, counter)
        .expect("repeated blinding derivation must succeed");
    assert_eq!(
        blinding.to_secret_bytes(),
        blinding_again.to_secret_bytes(),
        "blinding derivation must be deterministic"
    );

    let next_secret = Secret::from_seed(&input.seed, keyset_id, counter + 1)
        .expect("adjacent counter must derive");
    let next_blinding = SecretKey::from_seed(&input.seed, keyset_id, counter + 1)
        .expect("adjacent blinding counter must derive");
    assert_ne!(secret, next_secret, "adjacent counters must change the secret");
    assert_ne!(
        blinding.to_secret_bytes(),
        next_blinding.to_secret_bytes(),
        "adjacent counters must change the blinding factor"
    );

    let restored = PreMintSecrets::restore_batch(
        keyset_id,
        &input.seed,
        counter,
        counter + batch_len,
    )
    .expect("bounded restore batch must derive");
    assert_eq!(restored.len(), batch_len as usize);
    for (offset, pre_mint) in restored.iter().enumerate() {
        let expected_counter = counter + offset as u32;
        let expected_secret = Secret::from_seed(&input.seed, keyset_id, expected_counter)
            .expect("restored secret counter must derive");
        let expected_r = SecretKey::from_seed(&input.seed, keyset_id, expected_counter)
            .expect("restored blinding counter must derive");
        let (expected_blinded, _) = blind_message(
            &expected_secret.to_bytes(),
            Some(expected_r.clone()),
        )
        .expect("derived secret and scalar must blind");
        assert_eq!(pre_mint.secret, expected_secret);
        assert_eq!(pre_mint.r.to_secret_bytes(), expected_r.to_secret_bytes());
        assert_eq!(pre_mint.blinded_message.blinded_secret, expected_blinded);
        assert_eq!(pre_mint.amount, Amount::ZERO);
    }

    let amount = Amount::from(u64::from(input.amount));
    let fee_and_amounts = FeeAndAmounts::from((0, vec![1, 2, 4, 8, 16, 32, 64, 128]));
    let mut expected_amounts = amount
        .split(&fee_and_amounts)
        .expect("power-of-two denominations cover an eight-bit amount");
    expected_amounts.sort();
    let generated = PreMintSecrets::from_seed(
        keyset_id,
        counter,
        &input.seed,
        amount,
        &SplitTarget::None,
        &fee_and_amounts,
    )
    .expect("bounded deterministic premint generation must succeed");
    assert_eq!(generated.amounts(), expected_amounts);
    assert_eq!(generated.total_amount().expect("bounded sum"), amount);
    for (offset, pre_mint) in generated.iter().enumerate() {
        assert_eq!(
            pre_mint.secret,
            Secret::from_seed(&input.seed, keyset_id, counter + offset as u32)
                .expect("premint secret counter must derive")
        );
        assert_eq!(
            pre_mint.r.to_secret_bytes(),
            SecretKey::from_seed(&input.seed, keyset_id, counter + offset as u32)
                .expect("premint blinding counter must derive")
                .to_secret_bytes()
        );
    }
});
