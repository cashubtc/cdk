#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut00::{BlindSignature, BlindedMessage};

fuzz_target!(|data: &str| {
    // Fuzz BlindSignature parsing
    let _: Result<BlindSignature, _> = serde_json::from_str(data);

    // Fuzz BlindedMessage parsing
    let _: Result<BlindedMessage, _> = serde_json::from_str(data);

    // Fuzz arrays of these types
    let _: Result<Vec<BlindSignature>, _> = serde_json::from_str(data);
    let _: Result<Vec<BlindedMessage>, _> = serde_json::from_str(data);
});
