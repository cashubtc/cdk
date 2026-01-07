#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut03::SwapRequest;

fuzz_target!(|data: &str| {
    // Fuzz SwapRequest JSON deserialization
    // SwapRequest contains:
    // - inputs: Proofs (Vec<Proof>)
    // - outputs: Vec<BlindedMessage>
    // This is a main API entry point for untrusted data
    let _: Result<SwapRequest, _> = serde_json::from_str(data);
});
