#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::Token;

fuzz_target!(|data: &str| {
    let _ = Token::from_str(data);
});
