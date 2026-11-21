#![no_main]

use std::str::FromStr;

use cashu::Amount;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // Fuzz Amount string parsing
    let _ = Amount::from_str(data);

    // Fuzz Amount JSON parsing
    let _: Result<Amount, _> = serde_json::from_str(data);
});
