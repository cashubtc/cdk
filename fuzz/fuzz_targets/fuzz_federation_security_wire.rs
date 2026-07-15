#![no_main]

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::psbt::Psbt;
use bitcoin::Transaction;
use cdk_common::federation::{
    FederationConfig, FederationFrostDkgConfirmationRequest, FederationFrostDkgRound1Request,
    FederationFrostDkgRound2Request, FederationFrostSigningPackageRequest,
    FederationJournalCatchUpRequest, FederationJournalCatchUpResponse,
};
use libfuzzer_sys::fuzz_target;
use serde::{de::DeserializeOwned, Serialize};

const MAX_JSON_FUZZ_BYTES: usize = 2 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() <= MAX_JSON_FUZZ_BYTES {
        if let Ok(config) = serde_json::from_slice::<FederationConfig>(data) {
            let _ = config.validate();
        }
        round_trip_json::<FederationFrostDkgRound1Request>(data);
        round_trip_json::<FederationFrostDkgRound2Request>(data);
        round_trip_json::<FederationFrostDkgConfirmationRequest>(data);
        round_trip_json::<FederationFrostSigningPackageRequest>(data);
        if let Ok(request) = serde_json::from_slice::<FederationJournalCatchUpRequest>(data) {
            let _ = request.effective_limit(64, 256);
            round_trip_value(&request);
        }
        round_trip_json::<FederationJournalCatchUpResponse>(data);
    }

    if let Ok(transaction) = deserialize::<Transaction>(data) {
        assert_eq!(serialize(&transaction), data);
    }
    if let Ok(psbt) = Psbt::deserialize(data) {
        let encoded = psbt.serialize();
        let _ = Psbt::deserialize(&encoded).expect("serialized PSBT must deserialize");
    }
});

fn round_trip_json<T>(data: &[u8])
where
    T: DeserializeOwned + Serialize,
{
    if let Ok(value) = serde_json::from_slice::<T>(data) {
        round_trip_value(&value);
    }
}

fn round_trip_value<T>(value: &T)
where
    T: DeserializeOwned + Serialize,
{
    let encoded = serde_json::to_vec(value).expect("deserialized value must serialize");
    let _: T = serde_json::from_slice(&encoded).expect("serialized value must deserialize");
}
