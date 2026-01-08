#![no_main]

use libfuzzer_sys::fuzz_target;

use cashu::nuts::nut00::token::{Token, TokenV4};

fuzz_target!(|data: &[u8]| {
    let bytes = data.to_vec();

    // Fuzz Token::try_from(&Vec<u8>)
    // This tests:
    // - "crawB" prefix validation
    // - CBOR deserialization via ciborium
    // - Minimum length check (5 bytes)
    let _: Result<Token, _> = Token::try_from(&bytes);

    // Fuzz TokenV4::try_from(&Vec<u8>) directly
    // Same parsing but returns TokenV4 directly
    let _: Result<TokenV4, _> = TokenV4::try_from(&bytes);

    // Also try with the "crawB" prefix prepended to exercise CBOR parsing
    // with arbitrary data after the valid prefix
    let mut prefixed = b"crawB".to_vec();
    prefixed.extend_from_slice(data);
    let _: Result<Token, _> = Token::try_from(&prefixed);
    let _: Result<TokenV4, _> = TokenV4::try_from(&prefixed);
});
