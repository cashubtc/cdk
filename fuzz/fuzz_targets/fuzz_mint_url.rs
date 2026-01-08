#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::MintUrl;

fuzz_target!(|data: &str| {
    let _ = MintUrl::from_str(data);
});
