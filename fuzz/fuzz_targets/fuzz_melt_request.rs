#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut05::MeltRequest;

fuzz_target!(|data: &str| {
    // Fuzz MeltRequest JSON deserialization
    // MeltRequest<String> contains:
    // - quote: String (quote ID)
    // - inputs: Proofs (Vec<Proof>)
    // - outputs: Option<Vec<BlindedMessage>>
    // This is a main API entry point for untrusted data
    let _: Result<MeltRequest<String>, _> = serde_json::from_str(data);
});
