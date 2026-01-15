#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut00::Witness;

fuzz_target!(|data: &str| {
    // Fuzz Witness enum deserialization
    // The Witness enum uses #[serde(untagged)] which can have edge cases
    // It dispatches to either P2PKWitness or HTLCWitness based on structure
    let _: Result<Witness, _> = serde_json::from_str(data);

    // Also try deserializing as a Vec of witnesses
    let _: Result<Vec<Witness>, _> = serde_json::from_str(data);
});
