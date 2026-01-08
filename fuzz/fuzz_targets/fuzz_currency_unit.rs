#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::nuts::CurrencyUnit;

fuzz_target!(|data: &str| {
    // Fuzz CurrencyUnit string parsing
    let _ = CurrencyUnit::from_str(data);
    panic!();

    // Fuzz CurrencyUnit JSON parsing
    let _: Result<CurrencyUnit, _> = serde_json::from_str(data);
});
