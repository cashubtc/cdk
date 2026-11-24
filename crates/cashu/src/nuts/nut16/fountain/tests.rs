//! Compatibility with nut-fountain v0.1.0-alpha.0.

use std::str::FromStr;

use crate::nuts::nut16::fountain::{
    Error, FountainDecoder, FountainEncoder, FRAME_OVERHEAD, MAX_FRAGMENT_LENGTH,
    MAX_MESSAGE_LENGTH,
};
use crate::nuts::{Token, TokenFountainDecoder, TokenV4};
use serde::Deserialize;

const TOKEN: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex"))
        .collect()
}

#[derive(Deserialize)]
struct Fixture {
    name: String,
    size: usize,
    message: String,
    frames: Vec<String>,
    accepted: Vec<bool>,
    high_sequence_frames: Vec<String>,
}

#[test]
fn typescript_frames_match_and_recover_from_repairs_alone() {
    let fixtures: Vec<Fixture> =
        serde_json::from_str(include_str!("testdata/fountain.json")).expect("fixtures");
    for fixture in fixtures {
        let message = unhex(&fixture.message);
        let mut encoder = FountainEncoder::new(&message, fixture.size).expect("encoder");
        let mut decoder = FountainDecoder::default();
        let count = encoder.fragment_count();
        for (i, frame) in fixture.frames.iter().enumerate() {
            let frame = unhex(frame);
            assert_eq!(
                encoder.next_part().expect("frame"),
                frame,
                "{} sequence {}",
                fixture.name,
                i + 1
            );
            if i >= count {
                let accepted = decoder.receive(&frame).expect("repair");
                assert_eq!(
                    accepted,
                    fixture.accepted[i - count],
                    "{} rank",
                    fixture.name
                );
                let rank = decoder.independent_fragment_count();
                assert!(!decoder.receive(&frame).expect("duplicate"));
                assert_eq!(decoder.independent_fragment_count(), rank);
            }
        }
        assert_eq!(
            decoder.message(),
            Some(message.as_slice()),
            "{} recovery",
            fixture.name
        );
        assert_eq!(decoder.resolved_fragment_count(), Some(count));
        assert_eq!(decoder.progress(), 1.0);

        // High unsigned sequences exercise the exact wrapping selection rules.
        // Source frames then finish recovery if these three frames lack rank.
        decoder.reset();
        for frame in &fixture.high_sequence_frames {
            decoder.receive(&unhex(frame)).expect("high sequence");
        }
        for frame in fixture.frames[..count].iter().rev() {
            decoder.receive(&unhex(frame)).expect("reordered source");
        }
        assert_eq!(decoder.message(), Some(message.as_slice()));
    }
}

#[test]
fn normative_vectors_and_progress_distinguish_rank_from_resolved_fragments() {
    let single = unhex("4e46010000000001000000010000000355bc801d010203a3b35f2d");
    assert_eq!(
        FountainEncoder::new(&[1, 2, 3], 3)
            .expect("encoder")
            .next_part()
            .expect("frame"),
        single
    );
    let frames = [
        "4e460100000000050000000400000004e08ab900603bc78017",
        "4e460100000000020000000400000004e08ab9002028d66b47",
        "4e460100000000060000000400000004e08ab9006047a6a5cc",
        "4e460100000000030000000400000004e08ab90030a86e9a55",
    ];
    let mut decoder = FountainDecoder::default();
    assert_eq!(decoder.resolved_fragment_count(), None);
    assert_eq!(decoder.fragment_count(), 0);
    assert_eq!(decoder.progress(), 0.0);
    for (i, frame) in frames.iter().enumerate() {
        assert!(decoder.receive(&unhex(frame)).expect("receive"));
        assert_eq!(decoder.independent_fragment_count(), i + 1);
        assert_eq!(decoder.progress(), (i + 1) as f64 / 4.0);
        assert_eq!(decoder.resolved_fragment_count(), Some([0, 2, 2, 4][i]));
    }
    assert_eq!(decoder.message(), Some([0x10, 0x20, 0x30, 0x40].as_slice()));
}

#[test]
fn invalid_frames_and_mixed_transfers_preserve_state_even_after_completion() {
    let mut encoder = FountainEncoder::new(&[1, 2, 3, 4], 2).expect("encoder");
    let first = encoder.next_part().expect("frame");
    let second = encoder.next_part().expect("frame");
    let mut corrupt = first.clone();
    corrupt[20] ^= 1;
    let mut wrong_version = first.clone();
    wrong_version[2] = 2;
    let mut wrong_flags = first.clone();
    wrong_flags[3] = 1;
    let mut invalid = vec![
        vec![],
        vec![0; FRAME_OVERHEAD],
        vec![0; FRAME_OVERHEAD + MAX_FRAGMENT_LENGTH + 1],
        corrupt,
        wrong_version,
        wrong_flags,
    ];
    for (offset, value) in [(4, 0u32), (8, 0), (8, 257), (8, 1), (12, u32::MAX)] {
        let mut part = first.clone();
        part[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        invalid.push(part);
    }
    for (message, size) in [
        (vec![5, 6, 7, 8], 2),
        (vec![1, 2, 3, 4], 3),
        (vec![1, 2, 3], 2),
    ] {
        invalid.push(
            FountainEncoder::new(&message, size)
                .expect("foreign encoder")
                .next_part()
                .expect("foreign frame"),
        );
    }
    let mut decoder = FountainDecoder::default();
    decoder.receive(&first).expect("first");
    for complete in [false, true] {
        for frame in &invalid {
            assert!(decoder.receive(frame).is_err());
            assert_eq!(decoder.complete(), complete);
            assert_eq!(
                decoder.independent_fragment_count(),
                if complete { 2 } else { 1 }
            );
        }
        assert!(!decoder.receive(&first).expect("duplicate"));
        decoder.receive(&second).expect("second");
    }
    assert_eq!(decoder.message(), Some([1, 2, 3, 4].as_slice()));
    decoder.reset();
    assert_eq!(decoder.fragment_count(), 0);
    assert_eq!(decoder.message(), None);
    assert_eq!(decoder.independent_fragment_count(), 0);
    assert_eq!(decoder.resolved_fragment_count(), None);
}

#[test]
fn checksum_and_padding_failures_discard_only_the_final_equation() {
    // Independently constructed with Python struct.pack('>IIII') and zlib.crc32.
    // Both frames have valid frame checksums; the reconstructed message is invalid.
    for frame in [
        "4e46010000000001000000010000000355bc801d01020301e0793031",
        "4e46010000000001000000010000000300000000010203acc375e3",
    ] {
        let mut decoder = FountainDecoder::default();
        assert!(matches!(
            decoder.receive(&unhex(frame)),
            Err(Error::MessageIntegrityMismatch)
        ));
        assert!(!decoder.complete());
        assert_eq!(decoder.fragment_count(), 1);
        assert_eq!(decoder.independent_fragment_count(), 0);
        assert_eq!(decoder.progress(), 0.0);
    }
    let mut encoder = FountainEncoder::new(&[0x10, 0x20, 0x30, 0x40], 2).expect("encoder");
    let mut decoder = FountainDecoder::default();
    decoder
        .receive(&encoder.next_part().expect("first"))
        .expect("receive");
    let bad_final = unhex("4e460100000000020000000200000004e08ab9003041993e44c1");
    assert!(matches!(
        decoder.receive(&bad_final),
        Err(Error::MessageIntegrityMismatch)
    ));
    assert_eq!(decoder.independent_fragment_count(), 1);
    decoder
        .receive(&encoder.next_part().expect("correct second"))
        .expect("retry");
    assert!(decoder.complete());
}

#[test]
fn protocol_limits_and_empty_messages() {
    for size in [0, MAX_FRAGMENT_LENGTH + 1, usize::MAX] {
        assert!(matches!(
            FountainEncoder::new(&[], size),
            Err(Error::InvalidFragmentLength { .. })
        ));
    }
    assert!(matches!(
        FountainEncoder::new(&[0; 257], 1),
        Err(Error::TooManyFragments { .. })
    ));
    assert!(matches!(
        FountainEncoder::new(&vec![0; MAX_MESSAGE_LENGTH + 1], MAX_FRAGMENT_LENGTH),
        Err(Error::MessageTooLarge { .. })
    ));
    for message in [vec![], vec![0xa5; MAX_MESSAGE_LENGTH]] {
        let mut encoder =
            FountainEncoder::new(&message, MAX_FRAGMENT_LENGTH).expect("boundary encoder");
        let mut decoder = FountainDecoder::default();
        for _ in 0..encoder.fragment_count() {
            let frame = encoder.next_part().expect("frame");
            assert_eq!(frame.len(), FRAME_OVERHEAD + MAX_FRAGMENT_LENGTH);
            decoder.receive(&frame).expect("receive");
        }
        assert!(decoder.complete());
        assert_eq!(decoder.message(), Some(message.as_slice()));
    }
}

#[test]
fn token_api_uses_binary_v4_and_recovers_lost_frames() {
    let original = Token::from_str(TOKEN).expect("token");
    let v3 = Token::from_str(&original.to_v3_string()).expect("v3 token");
    let normalized = match &v3 {
        Token::TokenV3(token) => {
            Token::TokenV4(TokenV4::try_from(token.clone()).expect("normalize"))
        }
        _ => panic!("expected V3"),
    };
    for (input, expected) in [(&original, &original), (&v3, &normalized)] {
        let mut encoder = input.fountain_encoder(32).expect("encoder");
        let mut decoder = TokenFountainDecoder::default();
        assert!(!encoder.is_single_fragment());
        assert!(decoder.token().expect("incomplete").is_none());
        for _ in 0..512 {
            let frame = encoder.next_part().expect("frame");
            if encoder.current_index() % 2 == 0 {
                decoder.receive(&frame).expect("receive");
            }
            if decoder.complete() {
                break;
            }
        }
        assert_eq!(decoder.token().expect("token").as_ref(), Some(expected));
        assert_eq!(
            decoder.resolved_fragment_count(),
            Some(encoder.fragment_count())
        );
        assert_eq!(
            decoder.independent_fragment_count(),
            encoder.fragment_count()
        );
        assert_eq!(decoder.progress(), 1.0);
        decoder.reset();
        assert_eq!(decoder.fragment_count(), 0);
        let mut single = input.fountain_encoder(1024).expect("single");
        assert!(single.is_single_fragment());
        let part = single.next_part().expect("frame");
        assert_eq!(&part[20..25], b"crawB");
        decoder.receive(&part).expect("receive");
        assert_eq!(decoder.token().expect("token").as_ref(), Some(expected));
    }
}

#[test]
fn token_decoder_rejects_non_tokens_after_transport_completion() {
    for message in [
        b"invalid".as_slice(),
        TOKEN.as_bytes(),
        b"crawB\xff".as_slice(),
    ] {
        let mut decoder = TokenFountainDecoder::default();
        let mut encoder = FountainEncoder::new(message, 1024).expect("encoder");
        decoder
            .receive(&encoder.next_part().expect("frame"))
            .expect("valid transport");
        assert!(decoder.complete());
        assert!(decoder.token().is_err());
    }
}
