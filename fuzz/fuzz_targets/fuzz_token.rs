#![no_main]

use std::str::FromStr;

use cashu::Token;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let _ = Token::from_str(data);
});
