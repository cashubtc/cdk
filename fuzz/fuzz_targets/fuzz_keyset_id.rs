#![no_main]

use std::str::FromStr;

use cashu::nuts::nut02::{Id, ShortKeysetId};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // Fuzz full keyset Id parsing
    let _ = Id::from_str(data);

    // Fuzz short keyset Id parsing
    let _ = ShortKeysetId::from_str(data);
});
