//! Crates behind the node that tests of its adapters drive directly.
//!
//! `cdk-axum`'s multi-member tests build consensus messages, FROST sessions
//! and wallets themselves; they reach those crates through this module rather
//! than naming them as dependencies of the HTTP adapter.

pub use aleph_bft;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
pub use cdk_bitcoin_wallet;
pub use cdk_frost;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
pub use cdk_frost_bip32;
pub use musig2;
pub use parity_scale_codec;
