#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut12::{BlindSignatureDleq, ProofDleq};

fuzz_target!(|data: &str| {
    // Fuzz ProofDleq JSON deserialization
    // ProofDleq contains: e, s, r (all SecretKey types)
    let _: Result<ProofDleq, _> = serde_json::from_str(data);

    // Fuzz BlindSignatureDleq JSON deserialization
    // BlindSignatureDleq contains: e, s (SecretKey types)
    let _: Result<BlindSignatureDleq, _> = serde_json::from_str(data);
});
