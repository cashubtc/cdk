#![no_main]

use std::collections::BTreeSet;

use cdk_frost::{
    AggregateSignature, AuthorizationRequest, DkgContext, DkgRound1Package, DkgRound2Package,
    MemberId, NonceCommitment, NonceCommitmentSet, NonceRecord, RootKeyEpoch, RootPublicPackage,
    SignatureShare, SigningDerivation, SigningSessionContext,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    assert_canonical(data, RootPublicPackage::decode, |value| value.encode());
    assert_canonical(data, NonceCommitment::decode, |value| value.encode());
    assert_canonical(data, SignatureShare::decode, |value| value.encode());
    assert_canonical(data, SigningSessionContext::decode, |value| value.encode());
    assert_canonical(data, SigningDerivation::decode, |value| value.encode());
    assert_canonical(data, AuthorizationRequest::decode, |value| value.encode());
    assert_canonical(data, NonceCommitmentSet::decode, |value| value.encode());

    let context = dkg_context();
    assert_canonical(
        data,
        |bytes| DkgRound1Package::decode(&context, bytes),
        |value| value.encode(),
    );
    assert_canonical(
        data,
        |bytes| DkgRound2Package::decode(&context, member(2), bytes),
        |value| value.encode(),
    );

    if let Ok(record) = NonceRecord::from_bytes(data) {
        let encoded = record.to_bytes().expect("validated nonce record");
        let _ = NonceRecord::from_bytes(&encoded).expect("encoded nonce record");
    }
    if let Ok(bytes) = <[u8; 64]>::try_from(data) {
        let _ = AggregateSignature::from_bytes(bytes);
    }
});

fn assert_canonical<T, D, E>(data: &[u8], decode: D, encode: E)
where
    D: FnOnce(&[u8]) -> Result<T, cdk_frost::Error>,
    E: FnOnce(T) -> Vec<u8>,
{
    if let Ok(value) = decode(data) {
        assert_eq!(encode(value), data);
    }
}

fn dkg_context() -> DkgContext {
    DkgContext::new(
        [1; 32],
        [2; 32],
        RootKeyEpoch::new(1).expect("fixed root epoch"),
        2,
        BTreeSet::from([member(1), member(2), member(3)]),
    )
    .expect("fixed fuzz DKG context")
}

fn member(id: u16) -> MemberId {
    MemberId::new(id).expect("fixed non-zero member")
}
