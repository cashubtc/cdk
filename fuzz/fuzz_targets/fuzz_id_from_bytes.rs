#![no_main]

use cashu::nuts::nut02::Id;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Fuzz full keyset Id parsing from raw bytes
    let _ = Id::from_bytes(data);
});
