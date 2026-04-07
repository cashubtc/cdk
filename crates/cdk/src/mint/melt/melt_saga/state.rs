use cdk_common::nuts::CurrencyUnit;
use cdk_common::Amount;
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
/// transitions to this state. The `attempt_internal_settlement` and `make_payment`
/// methods are available.
///
/// Input proof Y values, blinded messages, operation, and fee breakdown are
/// persisted to the database during setup and retrieved from there during
/// finalization via the single shared finalization path.
pub struct SetupComplete {
    pub quote: MeltQuote,
}

/// Payment confirmed - has quote and payment result.
///
/// After successful payment (internal or external), the saga transitions to this state.
/// Only the `finalize` method is available, which delegates to the shared
/// `finalize_melt_quote` function — the single finalization path that handles
/// operation recording, saga deletion, and all cleanup atomically.
pub struct PaymentConfirmed {
    pub quote: MeltQuote,
    pub payment_result: MakePaymentResponse,
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
    Internal { amount: Amount<CurrencyUnit> },
    /// Payment requires external Lightning Network settlement.
    RequiresExternalPayment,
}
