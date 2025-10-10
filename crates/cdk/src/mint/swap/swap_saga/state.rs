use cdk_common::nuts::{BlindSignature, BlindedMessage};
use cdk_common::PublicKey;

#[derive(Debug, Clone, PartialEq)]
pub enum SwapState {
    Initial,
    SetupComplete {
        blinded_messages: Vec<BlindedMessage>,
        ys: Vec<PublicKey>,
    },
    Signed {
        blinded_messages: Vec<BlindedMessage>,
        signatures: Vec<BlindSignature>,
        ys: Vec<PublicKey>,
    },
    Completed,
}

impl SwapState {
    pub fn can_transition_to(&self, next: &SwapState) -> bool {
        use SwapState::*;

        matches!(
            (self, next),
            (Initial, SetupComplete { .. })
                | (SetupComplete { .. }, Signed { .. })
                | (Signed { .. }, Completed)
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            SwapState::Initial => "Initial",
            SwapState::SetupComplete { .. } => "SetupComplete",
            SwapState::Signed { .. } => "Signed",
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
        let setup_complete = SwapState::SetupComplete {
            blinded_messages: vec![],
            ys: vec![],
        };
        let signed = SwapState::Signed {
            blinded_messages: vec![],
            signatures: vec![],
            ys: vec![],
        };
        let completed = SwapState::Completed;

        assert!(initial.can_transition_to(&setup_complete));
        assert!(setup_complete.can_transition_to(&signed));
        assert!(signed.can_transition_to(&completed));
    }

    #[test]
    fn test_invalid_state_transitions() {
        let initial = SwapState::Initial;
        let signed = SwapState::Signed {
            blinded_messages: vec![],
            signatures: vec![],
            ys: vec![],
        };
        let completed = SwapState::Completed;

        // Cannot skip states
        assert!(!initial.can_transition_to(&signed));
        assert!(!initial.can_transition_to(&completed));

        // Cannot go backwards
        assert!(!completed.can_transition_to(&initial));
        assert!(!signed.can_transition_to(&initial));
    }
}
