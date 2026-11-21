#![no_main]

use cashu::PaymentRequest;
use libfuzzer_sys::fuzz_target;

// Fuzz the NUT-26 bech32m decoding with arbitrary strings
fuzz_target!(|data: &str| {
    let _ = PaymentRequest::from_bech32_string(data);
});
