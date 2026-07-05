#![no_main]

use cdk_common::federation::{FederationOperationEnvelope, FEDERATION_OPERATION_VERSION};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(envelope) = FederationOperationEnvelope::from_json_slice(data) else {
        return;
    };

    assert_eq!(envelope.version, FEDERATION_OPERATION_VERSION);
    let _ = envelope.ensure_admissible_size();
    let _ = envelope.operation_id();
});
