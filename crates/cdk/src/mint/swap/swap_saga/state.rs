/// State machine for swap operations.
///
/// The swap follows a strict linear progression:
/// Initial → SetupComplete → Signed → Completed
///
/// This enum tracks only the progress state. Actual data (blinded messages,
/// signatures, etc.) is stored in the SwapSaga struct to avoid duplication
/// and unnecessary cloning between state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapState {
    /// Initial state before any operations
    Initial,
    /// Setup complete: inputs reserved, outputs registered, balance verified
    SetupComplete,
    /// Blind signatures generated for outputs
    Signed,
    /// Swap finalized: signatures persisted, inputs marked spent
    Completed,
}

impl SwapState {
    /// Validates whether a state transition is allowed.
    ///
    /// Enforces the linear progression: Initial → SetupComplete → Signed → Completed
    /// Backward transitions and skipping states are not permitted.
    pub fn can_transition_to(&self, next: &SwapState) -> bool {
        use SwapState::*;

        matches!(
            (self, next),
            (Initial, SetupComplete) | (SetupComplete, Signed) | (Signed, Completed)
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            SwapState::Initial => "Initial",
            SwapState::SetupComplete => "SetupComplete",
            SwapState::Signed => "Signed",
            SwapState::Completed => "Completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_state_transitions() {
        let initial = SwapState::Initial;
        let setup_complete = SwapState::SetupComplete;
        let signed = SwapState::Signed;
        let completed = SwapState::Completed;

        assert!(initial.can_transition_to(&setup_complete));
        assert!(setup_complete.can_transition_to(&signed));
        assert!(signed.can_transition_to(&completed));
    }

    #[test]
    fn test_invalid_state_transitions() {
        let initial = SwapState::Initial;
        let setup_complete = SwapState::SetupComplete;
        let signed = SwapState::Signed;
        let completed = SwapState::Completed;

        // Cannot skip states
        assert!(!initial.can_transition_to(&signed));
        assert!(!initial.can_transition_to(&completed));
        assert!(!setup_complete.can_transition_to(&completed));

        // Cannot go backwards
        assert!(!completed.can_transition_to(&initial));
        assert!(!completed.can_transition_to(&setup_complete));
        assert!(!signed.can_transition_to(&initial));
        assert!(!signed.can_transition_to(&setup_complete));
    }
}
