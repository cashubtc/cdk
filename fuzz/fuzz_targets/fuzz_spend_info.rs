#![no_main]

//! Spend info (NUT-00, v3): JSON and V4 CBOR parsing, shape validation and
//! receive check 1 never panic, and JSON round-trips preserve the value.

use cashu::nuts::nut00::spend_info::{SpendInfo, SpendInfoV4};
use cashu::{PublicKey, SecretKey};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let secret =
        PublicKey::from_hex("02d310a4d661e3158e7d360617e739d6bacbf015431b24a43168db0ab99ef8f828")
            .expect("valid point");
    let receiver = SecretKey::from_slice(&[[0u8; 31].as_slice(), &[3]].concat()).expect("key");

    if let Ok(info) = serde_json::from_slice::<SpendInfo>(data) {
        let _ = info.validate();
        let _ = info.reconstruct(&secret, Some(&receiver));
        let json = serde_json::to_string(&info).expect("serialize");
        let again: SpendInfo = serde_json::from_str(&json).expect("round trip");
        assert_eq!(again, info);
    }
    if let Ok(info) = ciborium::de::from_reader::<SpendInfoV4, _>(data) {
        let _ = info.0.reconstruct(&secret, None);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&info, &mut bytes).expect("serialize");
    }
});
