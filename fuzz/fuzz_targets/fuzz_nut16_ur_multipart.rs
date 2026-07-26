#![no_main]

use arbitrary::Arbitrary;
use cashu::nuts::nut16::MAX_UR_PART_LENGTH;
use cashu::nuts::TokenUrDecoder;
use ciborium::value::{Integer, Value};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct MultipartInput {
    sequence: u32,
    fragment_count: u32,
    message_length: u32,
    checksum: u32,
    data: Vec<u8>,
    uppercase: bool,
}

fn integer(value: u32) -> Value {
    Value::Integer(Integer::from(value))
}

// Construct a syntactically valid multipart UR around arbitrary fountain
// metadata. This reaches the resource-limit checks that random strings rarely
// pass deeply enough to exercise.
fuzz_target!(|input: MultipartInput| {
    let MultipartInput {
        sequence,
        fragment_count,
        message_length,
        checksum,
        mut data,
        uppercase,
    } = input;
    data.truncate(MAX_UR_PART_LENGTH);
    let metadata = Value::Array(vec![
        integer(sequence),
        integer(fragment_count),
        integer(message_length),
        integer(checksum),
        Value::Bytes(data),
    ]);
    let mut cbor = Vec::new();
    if ciborium::ser::into_writer(&metadata, &mut cbor).is_err() {
        return;
    }

    let encoded = ur::encode(&cbor, &ur::Type::Bytes);
    let Some(payload) = encoded.strip_prefix("ur:bytes/") else {
        return;
    };
    let mut part = format!("ur:bytes/{sequence}-{fragment_count}/{payload}");
    if uppercase {
        part.make_ascii_uppercase();
    }

    let mut decoder = TokenUrDecoder::default();
    let _ = decoder.receive(&part);
    if decoder.complete() {
        let _ = decoder.token();
    }
});
