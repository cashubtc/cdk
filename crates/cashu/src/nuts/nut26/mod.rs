//! NUT-26: Payment Request Bech32m Encoding
//!
//! This module implements NUT-26, which provides bech32m encoding for Cashu payment requests.
//! NUT-26 is an alternative encoding to the JSON-based CREQ-A format (NUT-18), offering
//! improved QR code compatibility and more efficient encoding.
//!
//! The encoding methods are implemented as extensions to `PaymentRequest` from NUT-18.
//!
//! <https://github.com/cashubtc/nuts/blob/main/26.md>

mod encoding;
mod error;

pub use encoding::CREQ_B_HRP;
pub use error::Error;
