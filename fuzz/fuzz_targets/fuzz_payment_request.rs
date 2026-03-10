#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::PaymentRequest;

fuzz_target!(|data: &str| {
    let _ = PaymentRequest::from_str(data);
});
