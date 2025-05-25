//! Golomb-Coded Set filters

use serde::{Deserialize, Serialize};

/// Response to a GET filter request
#[derive(Serialize, Deserialize)]
pub struct GetFilterResponse {
    /// Number of items in the filter
    pub n: u32,
    /// Bit-length of the remainder
    pub p: u32,
    /// Inverse false positive rate
    pub m: u32,
    /// Compressed set. Bytes of the filter.
    pub content: String,
    /// Unix epoch in seconds when the filter was computed.
    pub timestamp: i64,
}
