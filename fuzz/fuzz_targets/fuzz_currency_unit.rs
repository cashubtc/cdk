#![no_main]

use std::str::FromStr;

use cashu::nuts::CurrencyUnit;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // Fuzz CurrencyUnit string parsing
    let _ = CurrencyUnit::from_str(data);

    // Fuzz CurrencyUnit JSON parsing
    let _: Result<CurrencyUnit, _> = serde_json::from_str(data);
});
