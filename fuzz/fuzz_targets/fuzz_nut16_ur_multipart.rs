#![no_main]

//! Stateful NUT-16 encoder/decoder fuzzing.
//!
//! Generates a valid V4 token, splits it into UR frames, reorders and
//! duplicates those frames, and asserts lossless reconstruction. A second
//! decoder receives one corrupted frame to retain malformed-frame coverage.

use cashu::nuts::{Token, TokenUrDecoder};
use cdk_fuzz::arbitrary_ext::TokenV4Arb;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct MultipartInput {
    token: TokenV4Arb,
    controls: Vec<u8>,
}

impl<'a> Arbitrary<'a> for MultipartInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let token = TokenV4Arb::arbitrary(u)?;
        Ok(Self {
            token,
            // Consuming the remainder without requiring a fixed number of
            // control bytes lets every valid TokenV4Arb corpus entry reach
            // the multipart encoder/decoder logic.
            controls: u.bytes(u.len())?.iter().copied().take(36).collect(),
        })
    }
}

fn shuffled_indices(count: usize, controls: &[u8]) -> Vec<usize> {
    let mut indices = (0..count).collect::<Vec<_>>();
    for (index, control) in controls.iter().copied().enumerate() {
        let left = index % count;
        let right = usize::from(control) % count;
        indices.swap(left, right);
    }
    indices
}

fn corrupt_part(part: &str) -> String {
    let mut bytes = part.as_bytes().to_vec();
    if let Some(index) = bytes.iter().rposition(u8::is_ascii_alphabetic) {
        bytes[index] = match bytes[index] {
            b'a' | b'A' => b'b',
            _ => b'a',
        };
    }
    String::from_utf8(bytes).expect("UR frames are ASCII")
}

fuzz_target!(|input: MultipartInput| {
    let fragment_control = u16::from_le_bytes([
        input.controls.first().copied().unwrap_or_default(),
        input.controls.get(1).copied().unwrap_or_default(),
    ]);
    let duplicate = input
        .controls
        .get(2)
        .is_some_and(|control| control & 1 != 0);
    let uppercase = input
        .controls
        .get(2)
        .is_some_and(|control| control & 2 != 0);
    let corrupt_index = input.controls.get(3).copied().unwrap_or_default();
    let order = input.controls.get(4..).unwrap_or_default();
    let token = Token::TokenV4(input.token.into_inner());
    // Keep frames small enough to force multipart encoding for most generated
    // tokens, but large enough to bound frame count and per-input work.
    let fragment_length = 32 + usize::from(fragment_control % 225);
    let mut encoder = token
        .ur_encoder(fragment_length)
        .expect("generated V4 token must be UR-encodable");
    let fragment_count = encoder.fragment_count();
    assert!(fragment_count > 0, "encoder must emit at least one frame");

    let mut parts = (0..fragment_count)
        .map(|_| {
            encoder
                .next_part()
                .expect("encoder must emit valid UR frames")
        })
        .collect::<Vec<_>>();
    if uppercase {
        for part in &mut parts {
            part.make_ascii_uppercase();
        }
    }

    let indices = shuffled_indices(parts.len(), order);
    let mut decoder = TokenUrDecoder::default();
    for index in indices.iter().copied() {
        decoder
            .receive(&parts[index])
            .expect("valid reordered UR frame must be accepted");
        if duplicate {
            decoder
                .receive(&parts[index])
                .expect("duplicate UR frame must be accepted");
        }
        if decoder.complete() {
            break;
        }
    }
    assert!(
        decoder.complete(),
        "all systematic frames must reconstruct token"
    );
    assert_eq!(
        decoder
            .token()
            .expect("complete decoder must return a token"),
        Some(token),
        "UR encoder/decoder round-trip mismatch"
    );

    let corrupt_index = usize::from(corrupt_index) % parts.len();
    parts[corrupt_index] = corrupt_part(&parts[corrupt_index]);
    let mut corrupted_decoder = TokenUrDecoder::default();
    for index in indices {
        let _ = corrupted_decoder.receive(&parts[index]);
        if corrupted_decoder.complete() {
            let _ = corrupted_decoder.token();
            break;
        }
    }
});
