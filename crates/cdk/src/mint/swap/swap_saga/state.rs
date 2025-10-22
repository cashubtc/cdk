use cdk_common::nuts::{BlindSignature, BlindedMessage};
use cdk_common::PublicKey;

/// Initial state - no data yet.
///
/// The swap saga starts in this state. Only the `setup_swap` method is available.
pub struct Initial;

/// Setup complete - has blinded messages and input Y values.
///
/// After successful setup, the saga transitions to this state.
/// Only the `sign_outputs` method is available.
pub struct SetupComplete {
    pub blinded_messages: Vec<BlindedMessage>,
    pub ys: Vec<PublicKey>,
}

/// Signed state - has everything including signatures.
///
/// After successful signing, the saga transitions to this state.
/// Only the `finalize` method is available.
pub struct Signed {
    pub blinded_messages: Vec<BlindedMessage>,
    pub ys: Vec<PublicKey>,
    pub signatures: Vec<BlindSignature>,
}
