#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoin::secp256k1::{rand, All, Secp256k1};
use once_cell::sync::Lazy;

pub mod hex;

#[cfg(target_arch = "wasm32")]
use instant::SystemTime;

#[cfg(target_arch = "wasm32")]
const UNIX_EPOCH: SystemTime = SystemTime::UNIX_EPOCH;

/// Secp256k1 global context
pub static SECP256K1: Lazy<Secp256k1<All>> = Lazy::new(|| {
    let mut ctx = Secp256k1::new();
    let mut rng = rand::thread_rng();
    ctx.randomize(&mut rng);
    ctx
});

/// Seconds since unix epoch
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, thiserror::Error)]
/// Error type for serialization
pub enum CborError {
    /// CBOR serialization error
    #[error("CBOR serialization error")]
    Cbor(#[from] ciborium::ser::Error<std::io::Error>),

    /// CBOR diagnostic notation error
    #[error("CBOR diagnostic notation error: {0}")]
    CborDiag(#[from] cbor_diag::Error),
}

/// Serializes a struct to the CBOR diagnostic notation.
///
/// See <https://www.rfc-editor.org/rfc/rfc8949.html#name-diagnostic-notation>
pub fn serialize_to_cbor_diag<T: serde::Serialize>(data: &T) -> Result<String, CborError> {
    let mut cbor_buffer = Vec::new();
    ciborium::ser::into_writer(data, &mut cbor_buffer)?;

    let diag = cbor_diag::parse_bytes(&cbor_buffer)?;
    Ok(diag.to_diag_pretty())
}
