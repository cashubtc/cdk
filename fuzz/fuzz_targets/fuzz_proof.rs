#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut00::{Proof, Witness};

fuzz_target!(|data: &str| {
    // Fuzz single Proof parsing
    let _: Result<Proof, _> = serde_json::from_str(data);

    // Fuzz Proofs array parsing
    let _: Result<Vec<Proof>, _> = serde_json::from_str(data);

    // Fuzz Witness parsing (embedded in proofs)
    let _: Result<Witness, _> = serde_json::from_str(data);
});
