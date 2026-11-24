//! Result of advancing one federated payment by at most one step.

/// What one advancement attempt of a federated mint or melt quote achieved.
///
/// Payment drivers return it; the node's payment scheduler uses it to pick the
/// next wake-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationPaymentAdvancement {
    /// A consensus-visible step was submitted and accepted.
    Progressed,
    /// The payment waits for an external event, such as an incoming payment.
    AwaitingExternalEvidence,
    /// The payment waits for the custody backend to report an update.
    AwaitingBackendNotification,
    /// The payment waits for other members' evidence or signatures.
    AwaitingFederationEvidence,
    /// This member is not a selected observer and waits for a fallback round.
    AwaitingObserverFallback,
    /// The payment reached a terminal state.
    Complete,
}
