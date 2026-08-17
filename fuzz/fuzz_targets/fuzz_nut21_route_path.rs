#![no_main]

//! Fuzz NUT-21 `RoutePath::from_str` and `matching_route_paths`.
//!
//! These functions do prefix stripping, wildcard handling, and payment-method
//! normalization on arbitrary strings.  Malformed input (wildcards in the
//! middle, empty prefixes, unicode) could trigger panics.

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut21::{matching_route_paths, RoutePath};

fuzz_target!(|data: &str| {
    // Fuzz RoutePath FromStr (prefix stripping, wildcard, normalization)
    let _ = RoutePath::from_str(data);

    // Fuzz matching_route_paths (wildcard expansion, exact matching)
    let _ = matching_route_paths(data);

    // Fuzz RoutePath JSON round-trip
    let _: Result<RoutePath, _> = serde_json::from_str(data);
});
