#![no_main]

use cashu::nuts::nut00::CurrencyUnit;
use cashu::nuts::nut02::{Id, KeySetInfo, ShortKeysetId};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // We need enough data to at least form a version byte and a prefix
    if data.len() < 2 {
        return;
    }

    // Create a dummy V1 keyset (7 bytes ID)
    // version 00 + 7 bytes
    let v1_bytes = [0u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
    let v1_id = match Id::from_bytes(&v1_bytes) {
        Ok(id) => id,
        Err(_) => return,
    };

    let v1_info = KeySetInfo {
        id: v1_id,
        unit: CurrencyUnit::Sat,
        active: true,
        input_fee_ppk: 0,
        final_expiry: None,
    };

    let keysets = vec![v1_info];

    // Attempt to resolve the short keyset ID from fuzzer data
    if let Ok(short_id) = ShortKeysetId::from_bytes(data) {
        // This is expected to panic if data starts with 0x01 and has > 7 bytes of prefix
        let _ = Id::from_short_keyset_id(&short_id, &keysets);
    }
});
