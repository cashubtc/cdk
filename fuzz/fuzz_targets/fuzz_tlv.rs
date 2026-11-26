#![no_main]

//! NUT-00 TLV field streams and minimal integers: reading never panics, a
//! decoded integer re-encodes to its bytes, and a read stream re-encodes to
//! its input.

use cashu::nuts::nut00::tlv::{decode_uint, encode_uint, read_field_stream, write_record};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = decode_uint(data) {
        assert_eq!(encode_uint(value), data);
    }
    if let Ok(records) = read_field_stream(data) {
        let mut out = Vec::new();
        for record in records {
            write_record(&mut out, record.record_type, record.value).expect("fits");
        }
        assert_eq!(out, data);
    }
});
