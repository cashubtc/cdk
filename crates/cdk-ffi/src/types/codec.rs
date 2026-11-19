//! Shared JSON decoding for the exported codecs.

use serde::de::DeserializeOwned;

use crate::error::FfiError;

/// Decodes the protocol encoding, falling back to the shape this crate's own
/// encoder wrote before these codecs moved to `N`, so a record a host persisted
/// then still loads.
pub(crate) fn decode_json<N, F>(json: &str) -> Result<F, FfiError>
where
    N: DeserializeOwned,
    F: DeserializeOwned + From<N>,
{
    let protocol = match serde_json::from_str::<N>(json) {
        Ok(value) => return Ok(value.into()),
        Err(err) => err,
    };

    serde_json::from_str::<F>(json).map_err(|legacy| {
        FfiError::internal(format!(
            "not the protocol encoding ({protocol}), and not a record this crate encoded ({legacy})"
        ))
    })
}
