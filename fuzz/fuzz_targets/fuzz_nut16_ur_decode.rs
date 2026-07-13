#![no_main]

use cashu::nuts::TokenUrDecoder;
use libfuzzer_sys::fuzz_target;

// Treat NUL-separated strings as consecutive scanner frames so the target
// covers both individual parsing and stateful fountain reconstruction.
fuzz_target!(|data: &str| {
    let mut decoder = TokenUrDecoder::default();

    for part in data.split('\0').take(32) {
        let _ = decoder.receive(part);
        if decoder.complete() {
            let _ = decoder.token();
            break;
        }
    }
});
