#![no_main]

//! Fuzz NUT-21 `ClearAuthSettings` custom deserialization.
//!
//! The custom `Deserialize` impl expands wildcard patterns via
//! `matching_route_paths`, so adversarial JSON with wildcard strings
//! exercises both the serde layer and the route-path parser.

use cashu::nuts::ClearAuthSettings;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    // Fuzz ClearAuthSettings JSON deserialization (custom impl with
    // wildcard pattern expansion)
    let _: Result<ClearAuthSettings, _> = serde_json::from_str(data);
});
