#![no_main]

use std::str::FromStr;

use cashu::PaymentRequest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let _ = PaymentRequest::from_str(data);
});
