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

/// Setup complete - has quote ready for payment.
///
/// After successful setup (proofs reserved, quote state set to Pending), the saga
/// transitions to this state. No payment has been attempted yet. The
/// `attempt_internal_settlement` and `make_payment` methods are available.
///
/// Input proof Y values, blinded messages, operation, and fee breakdown are
/// persisted to the database during setup and retrieved from there during
/// finalization via the single shared finalization path.
pub struct SetupComplete {
    pub quote: MeltQuote,
}

/// Payment confirmed - has quote and payment result.
///
/// After successful payment (internal or external), the durable `Finalizing`
/// handoff is already persisted in the database and the saga transitions to
/// this state. Only the `finalize` method is available, which delegates to the
/// shared `finalize_melt_quote` function — the single finalization path that
/// handles operation recording, saga deletion, and all cleanup atomically.
pub struct PaymentConfirmed {
    pub quote: MeltQuote,
    pub payment_result: MakePaymentResponse,
}

/// Outcome of attempting internal settlement for a melt operation.
///
/// This typestate branch enforces the invariant that `MeltSaga<SetupComplete>`
/// never represents an already-settled internal melt:
/// - Internal settlement committed atomically (mint quote credited, melt quote
///   marked `Paid`, saga transitioned `SetupComplete -> Finalizing`) → saga
///   advances directly to [`PaymentConfirmed`]
/// - Not an internal payment → saga remains in [`SetupComplete`] for external
///   payment dispatch
pub enum InternalSettlementOutcome {
    /// Payment settled internally on this mint; saga is ready for finalization.
    Settled(Box<super::MeltSaga<PaymentConfirmed>>),
    /// Payment requires external settlement via the payment backend.
    RequiresExternalPayment(Box<super::MeltSaga<SetupComplete>>),
}
