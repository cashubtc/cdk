#![no_main]

use std::str::FromStr;

use cashu::MintUrl;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let _ = MintUrl::from_str(data);
});
