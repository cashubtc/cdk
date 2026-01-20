#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut10::Secret as Nut10Secret;
use cashu::nuts::nut11::{Conditions, SpendingConditions};
use cashu::secret::Secret;

fuzz_target!(|data: &str| {
    // Fuzz HTLC creation with preimage (hex string validation)
    // This tests the 32-byte preimage requirement and hex decoding
    let _ = SpendingConditions::new_htlc(data.to_string(), None);

    // Fuzz HTLC creation from hash string directly
    let _ = SpendingConditions::new_htlc_hash(data, None);

    // Fuzz SpendingConditions extraction from raw Secret
    // This exercises the full parsing pipeline: Secret -> Nut10Secret -> SpendingConditions
    if let Ok(secret) = Secret::from_str(data) {
        let _: Result<SpendingConditions, _> = SpendingConditions::try_from(&secret);
    }

    // Fuzz SpendingConditions from Nut10Secret
    if let Ok(nut10_secret) = serde_json::from_str::<Nut10Secret>(data) {
        let _: Result<SpendingConditions, _> = SpendingConditions::try_from(nut10_secret);
    }

    // Fuzz Conditions JSON deserialization directly
    let _: Result<Conditions, _> = serde_json::from_str(data);

    // Fuzz Conditions from tags (Vec<Vec<String>>)
    // This tests the tag parsing logic for locktime, pubkeys, refund_keys, n_sigs, sigflag
    if let Ok(tags) = serde_json::from_str::<Vec<Vec<String>>>(data) {
        let _: Result<Conditions, _> = Conditions::try_from(tags);
    }
});
