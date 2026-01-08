#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut14::HTLCWitness;

fuzz_target!(|data: &str| {
    // Fuzz HTLCWitness JSON deserialization
    // This tests the preimage and signatures field parsing
    if let Ok(witness) = serde_json::from_str::<HTLCWitness>(data) {
        // If we successfully parsed an HTLCWitness, fuzz the preimage_data() method
        // This validates:
        // - Hex decoding of the preimage
        // - Exactly 32 bytes requirement
        let _ = witness.preimage_data();
    }

    // Fuzz preimage_data() directly with arbitrary preimage strings
    // This exercises the hex validation and size checking with raw input
    let witness = HTLCWitness {
        preimage: data.to_string(),
        signatures: None,
    };
    let _ = witness.preimage_data();

    // Fuzz with signatures field populated
    // Test various signature list formats
    if let Ok(sigs) = serde_json::from_str::<Vec<String>>(data) {
        let witness_with_sigs = HTLCWitness {
            preimage: String::new(),
            signatures: Some(sigs),
        };
        let _ = witness_with_sigs.preimage_data();
    }
});
