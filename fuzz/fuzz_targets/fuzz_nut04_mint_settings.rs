#![no_main]

//! Fuzz NUT-04 `MintMethodSettings` custom deserialization.
//!
//! The hand-written `Visitor` impl walks arbitrary JSON maps, handles
//! `options` merging with top-level fields, and detects duplicates.
//! This fuzz target feeds arbitrary JSON strings to that code path.

use libfuzzer_sys::fuzz_target;

use cashu::nuts::MintMethodSettings;

fuzz_target!(|data: &str| {
    // Fuzz MintMethodSettings JSON deserialization (custom Visitor)
    let _: Result<MintMethodSettings, _> = serde_json::from_str(data);

    // Fuzz the full NUT-04 Settings wrapper as well
    let _: Result<cashu::nuts::NUT04Settings, _> = serde_json::from_str(data);
});
