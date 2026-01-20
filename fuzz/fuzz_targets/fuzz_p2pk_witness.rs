#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut11::P2PKWitness;

fuzz_target!(|data: &str| {
    // Fuzz P2PKWitness JSON deserialization
    // This tests signature list parsing with various formats
    let _: Result<P2PKWitness, _> = serde_json::from_str(data);

    // Fuzz with arbitrary signature strings
    // Test various invalid signature formats
    if let Ok(sigs) = serde_json::from_str::<Vec<String>>(data) {
        let witness = P2PKWitness { signatures: sigs };
        // Verify the witness can be serialized back
        let _ = serde_json::to_string(&witness);
    }
});
