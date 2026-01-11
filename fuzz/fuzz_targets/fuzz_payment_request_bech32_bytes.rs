#![no_main]

use libfuzzer_sys::fuzz_target;

use bitcoin::bech32::{Bech32m, Hrp};
use cashu::PaymentRequest;

// Fuzz the NUT-26 TLV parser by constructing valid bech32m encoding
// around arbitrary bytes. This bypasses bech32 charset validation to
// directly stress-test the TLV parsing logic.
fuzz_target!(|data: &[u8]| {
    // Construct a valid bech32m string with "creqb" HRP and fuzzed payload
    let hrp = Hrp::parse("creqb").unwrap();
    if let Ok(encoded) = bitcoin::bech32::encode::<Bech32m>(hrp, data) {
        let _ = PaymentRequest::from_bech32_string(&encoded);
    }
});
