#![no_main]

//! Nutroot witnesses (NUT-00/10, v3): parsing and verification of arbitrary
//! witness JSON against a fixed secret and digest never panics, and a
//! parsed witness round-trips through its JSON.

use std::str::FromStr;

use cashu::nuts::nut10::nutroot::{verify_witness, NutrootWitness};
use cashu::PublicKey;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &str| {
    let secret =
        PublicKey::from_hex("02d310a4d661e3158e7d360617e739d6bacbf015431b24a43168db0ab99ef8f828")
            .expect("valid point");
    let _ = NutrootWitness::from_quote_signature(data);
    if let Ok(witness) = NutrootWitness::from_str(data) {
        let again = NutrootWitness::from_str(&witness.to_string()).expect("round trip");
        assert_eq!(again, witness);
        let _ = verify_witness(&secret, &witness, &[7u8; 32], 1_755_561_600);
    }
});
