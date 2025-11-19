use cdk_common::mint::Operation;
use cdk_common::nuts::{BlindSignature, BlindedMessage};
use cdk_common::PublicKey;
use uuid::Uuid;

/// Initial state - only has operation ID.
///
/// The swap saga starts in this state. Only the `setup_swap` method is available.
/// The operation ID is generated upfront but the full Operation (with amounts) is created during setup.
pub struct Initial {
    pub operation_id: Uuid,
}

/// Setup complete - has blinded messages, input Y values, and the Operation with actual amounts.
///
/// After successful setup, the saga transitions to this state.
/// Only the `sign_outputs` method is available.
pub struct SetupComplete {
    pub blinded_messages: Vec<BlindedMessage>,
    pub ys: Vec<PublicKey>,
    pub operation: Operation,
    pub fee_breakdown: crate::fees::ProofsFeeBreakdown,
}

/// Signed state - has everything including signatures.
///
/// After successful signing, the saga transitions to this state.
/// Only the `finalize` method is available.
pub struct Signed {
    pub blinded_messages: Vec<BlindedMessage>,
    pub ys: Vec<PublicKey>,
    pub signatures: Vec<BlindSignature>,
    pub operation: Operation,
    pub fee_breakdown: crate::fees::ProofsFeeBreakdown,
}
