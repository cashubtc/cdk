#![no_main]

use cdk_frost::{NonceCommitment, RootPublicPackage, SignatureShare};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(root) = RootPublicPackage::decode(data) {
        assert_eq!(root.encode(), data);
    }
    if let Ok(commitment) = NonceCommitment::decode(data) {
        assert_eq!(commitment.encode(), data);
    }
    if let Ok(share) = SignatureShare::decode(data) {
        assert_eq!(share.encode(), data);
    }
});
