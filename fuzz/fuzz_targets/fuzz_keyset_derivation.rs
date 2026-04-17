#![no_main]

//! Fuzzing for NUT-02 keyset ID derivation and NUT-13 deterministic
//! secret/blinding-factor derivation.
//!
//! Goals:
//!   - NUT-02: exercise `Id::v1_from_keys` and `Id::v2_from_data` over
//!     arbitrary `Keys` maps, plus round-trip through `Id::to_bytes` /
//!     `Id::from_bytes` and `FromStr` / `Display`.
//!   - NUT-13: exercise deterministic derivation of `Secret` and `SecretKey`
//!     from a 64-byte seed over both V1 and V2 keyset ids, asserting that
//!     derivation is stable (same inputs -> same output).

use std::collections::BTreeMap;
use std::str::FromStr;

use cashu::nuts::nut00::CurrencyUnit;
use cashu::nuts::nut01::Keys;
use cashu::secret::Secret as SecretString;
use cashu::{Amount, Id, PublicKey, SecretKey};
use cdk_fuzz::pubkey_from;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    // Seed amounts + pubkey bytes for a Keys map.
    key_entries: Vec<(u64, [u8; 32])>,
    // NUT-02 v2 inputs.
    unit_tag: u8,
    custom_unit: Option<String>,
    input_fee_ppk: u64,
    expiry: Option<u64>,
    // NUT-13 inputs.
    seed: [u8; 64],
    counter: u32,
    use_v2_id: bool,
    // Fuzz `Id::from_bytes` / `FromStr` with raw data too.
    raw_id_bytes: Vec<u8>,
    raw_id_str: String,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        Ok(Self {
            key_entries: {
                let n = u.int_in_range(1..=8)?;
                (0..n)
                    .map(|_| Ok((u64::arbitrary(u)?, <[u8; 32]>::arbitrary(u)?)))
                    .collect::<libfuzzer_sys::arbitrary::Result<_>>()?
            },
            unit_tag: u.arbitrary()?,
            custom_unit: Option::<String>::arbitrary(u)?,
            input_fee_ppk: u.arbitrary()?,
            expiry: Option::<u64>::arbitrary(u)?,
            seed: u.arbitrary()?,
            counter: u.arbitrary()?,
            use_v2_id: u.arbitrary()?,
            raw_id_bytes: Vec::<u8>::arbitrary(u)?,
            raw_id_str: String::arbitrary(u)?,
        })
    }
}

fn currency_unit_from(tag: u8, custom: &Option<String>) -> CurrencyUnit {
    match tag % 4 {
        0 => CurrencyUnit::Sat,
        1 => CurrencyUnit::Msat,
        2 => CurrencyUnit::Usd,
        _ => match custom {
            Some(s) if !s.is_empty() => CurrencyUnit::from_str(s).unwrap_or(CurrencyUnit::Sat),
            _ => CurrencyUnit::Sat,
        },
    }
}

fuzz_target!(|input: Input| {
    // Build a `Keys` map out of the fuzz entries.
    let mut map: BTreeMap<Amount, PublicKey> = BTreeMap::new();
    for (amt, seed) in &input.key_entries {
        map.insert(Amount::from(*amt), pubkey_from(*seed));
    }
    if map.is_empty() {
        return;
    }
    let keys = Keys::new(map);

    // --- NUT-02: V1 id derivation ---
    let id_v1 = Id::v1_from_keys(&keys);
    // Round-trip byte form.
    let bytes = id_v1.to_bytes();
    if let Ok(roundtrip) = Id::from_bytes(&bytes) {
        assert_eq!(roundtrip, id_v1);
    }
    // Round-trip string form.
    let s = id_v1.to_string();
    if let Ok(parsed) = Id::from_str(&s) {
        assert_eq!(parsed, id_v1);
    }

    // --- NUT-02: V2 id derivation ---
    let unit = currency_unit_from(input.unit_tag, &input.custom_unit);
    let id_v2 = Id::v2_from_data(&keys, &unit, input.input_fee_ppk, input.expiry);
    let v2_bytes = id_v2.to_bytes();
    if let Ok(roundtrip) = Id::from_bytes(&v2_bytes) {
        assert_eq!(roundtrip, id_v2);
    }
    // Deterministic: same inputs -> same id.
    let id_v2_again = Id::v2_from_data(&keys, &unit, input.input_fee_ppk, input.expiry);
    assert_eq!(id_v2, id_v2_again);

    // Fuzz raw parsers.
    let _ = Id::from_bytes(&input.raw_id_bytes);
    let _ = Id::from_str(&input.raw_id_str);

    // --- NUT-13: deterministic secret + blinding factor derivation ---
    let id_for_nut13 = if input.use_v2_id { id_v2 } else { id_v1 };

    let derived_secret = SecretString::from_seed(&input.seed, id_for_nut13, input.counter);
    let derived_r = SecretKey::from_seed(&input.seed, id_for_nut13, input.counter);

    // Stability check: re-deriving with identical inputs must match.
    if let Ok(ref s1) = derived_secret {
        let s2 = SecretString::from_seed(&input.seed, id_for_nut13, input.counter)
            .expect("second derivation with same inputs must succeed when first did");
        assert_eq!(s1.as_bytes(), s2.as_bytes());
    }
    if let Ok(ref r1) = derived_r {
        let r2 = SecretKey::from_seed(&input.seed, id_for_nut13, input.counter)
            .expect("second r derivation must succeed when first did");
        assert_eq!(r1.as_secret_bytes(), r2.as_secret_bytes());
    }
});
