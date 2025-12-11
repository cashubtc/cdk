use cdk_common::mint::Operation;
use cdk_common::nuts::BlindedMessage;
use cdk_common::{Amount, PublicKey};
use uuid::Uuid;

use crate::cdk_payment::MakePaymentResponse;
use crate::mint::MeltQuote;

/// Initial state - only has operation ID.
///
/// The melt saga starts in this state. Only the `setup_melt` method is available.
/// The operation ID is generated upfront but the full Operation (with amounts) is created during setup.
pub struct Initial {
    pub operation_id: Uuid,
}

/// Setup complete - has quote, input Ys, blinded messages, and the Operation with actual amounts.
///
/// After successful setup, the saga transitions to this state.
/// The `attempt_internal_settlement` and `make_payment` methods are available.
pub struct SetupComplete {
    pub quote: MeltQuote,
    pub input_ys: Vec<PublicKey>,
    pub blinded_messages: Vec<BlindedMessage>,
    pub operation: Operation,
    pub fee_breakdown: crate::fees::ProofsFeeBreakdown,
}

/// Payment confirmed - has everything including payment result.
///
/// After successful payment (internal or external), the saga transitions to this state.
/// Only the `finalize` method is available.
pub struct PaymentConfirmed {
    pub quote: MeltQuote,
    pub input_ys: Vec<PublicKey>,
    #[allow(dead_code)] // Stored for completeness, accessed from DB in finalize
    pub blinded_messages: Vec<BlindedMessage>,
    pub payment_result: MakePaymentResponse,
    pub operation: Operation,
    pub fee_breakdown: crate::fees::ProofsFeeBreakdown,
}

/// Result of attempting internal settlement for a melt operation.
///
/// This enum represents the decision point in the melt flow:
/// - Internal settlement succeeded → skip external Lightning payment
/// - External payment required → proceed with Lightning Network call
#[derive(Debug, Clone)]
pub enum SettlementDecision {
    /// Payment was settled internally (melt-to-mint on the same mint).
    /// Contains the amount that was settled.
    Internal { amount: Amount },
    /// Payment requires external Lightning Network settlement.
    RequiresExternalPayment,
}
