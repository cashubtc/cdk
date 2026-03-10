#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut10::Secret as Nut10Secret;
use cashu::secret::Secret;

fuzz_target!(|data: &str| {
    // Fuzz the NUT-10 secret JSON parsing (complex structured data)
    let _: Result<Nut10Secret, _> = serde_json::from_str(data);

    // Also try the conversion path: raw secret -> nut10 secret
    if let Ok(secret) = Secret::from_str(data) {
        let _: Result<Nut10Secret, _> = Nut10Secret::try_from(&secret);
    }
});
