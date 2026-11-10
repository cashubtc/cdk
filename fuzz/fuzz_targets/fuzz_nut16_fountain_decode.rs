#![no_main]

use cashu::nuts::TokenFountainDecoder;
use libfuzzer_sys::fuzz_target;

// Each input contains up to 32 frames prefixed with a big-endian u16 length.
// Also try the entire input as a frame to cover standalone scanner input.
fuzz_target!(|data: &[u8]| {
    let mut decoder = TokenFountainDecoder::default();
    let _ = decoder.receive(data);
    let _ = decoder.token();
    decoder.reset();
    let mut remaining = data;
    for _ in 0..32 {
        if remaining.len() < 2 {
            break;
        }
        let length = usize::from(u16::from_be_bytes([remaining[0], remaining[1]]));
        remaining = &remaining[2..];
        if length > remaining.len() {
            break;
        }
        let _ = decoder.receive(&remaining[..length]);
        let _ = decoder.token();
        let _ = decoder.resolved_fragment_count();
        remaining = &remaining[length..];
    }
});
