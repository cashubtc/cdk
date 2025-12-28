//! State transition rules

use cashu::{MeltQuoteState, State};

/// State transition Error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Pending Token
    #[error("Token already pending for another update")]
    Pending,
    /// Already spent
    #[error("Token already spent")]
    AlreadySpent,
    /// Invalid transition
    #[error("Invalid transition: From {0} to {1}")]
    InvalidTransition(State, State),
    /// Already paid
    #[error("Quote already paid")]
    AlreadyPaid,
    /// Invalid transition
    #[error("Invalid melt quote state transition: From {0} to {1}")]
    InvalidMeltQuoteTransition(MeltQuoteState, MeltQuoteState),
}

#[inline]
/// Check if the state transition is allowed
pub fn check_state_transition(current_state: State, new_state: State) -> Result<(), Error> {
    let is_valid_transition = match current_state {
        State::Unspent => matches!(new_state, State::Pending | State::Spent),
        State::Pending => matches!(new_state, State::Unspent | State::Spent),
        // Any other state shouldn't be updated by the mint, and the wallet does not use this
        // function
        _ => false,
    };

    if !is_valid_transition {
        Err(match current_state {
            State::Pending => Error::Pending,
            State::Spent => Error::AlreadySpent,
            _ => Error::InvalidTransition(current_state, new_state),
        })
    } else {
        Ok(())
    }
}

#[inline]
/// Check if the melt quote state transition is allowed
///
/// Valid transitions:
/// - Unpaid -> Pending, Failed
/// - Pending -> Unpaid, Paid, Failed
/// - Paid -> (no transitions allowed)
/// - Failed -> Pending
pub fn check_melt_quote_state_transition(
    current_state: MeltQuoteState,
    new_state: MeltQuoteState,
) -> Result<(), Error> {
    let is_valid_transition = match current_state {
        MeltQuoteState::Unpaid => {
            matches!(new_state, MeltQuoteState::Pending | MeltQuoteState::Failed)
        }
        MeltQuoteState::Pending => matches!(
            new_state,
            MeltQuoteState::Unpaid | MeltQuoteState::Paid | MeltQuoteState::Failed
        ),
        MeltQuoteState::Failed => {
            matches!(new_state, MeltQuoteState::Pending | MeltQuoteState::Unpaid)
        }
        MeltQuoteState::Paid => false,
        MeltQuoteState::Unknown => true,
    };

    if !is_valid_transition {
        Err(match current_state {
            MeltQuoteState::Pending => Error::Pending,
            MeltQuoteState::Paid => Error::AlreadyPaid,
            _ => Error::InvalidMeltQuoteTransition(current_state, new_state),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod proof_state_transitions {
        use super::*;

        #[test]
        fn unspent_to_pending_is_valid() {
            assert!(check_state_transition(State::Unspent, State::Pending).is_ok());
        }

        #[test]
        fn unspent_to_spent_is_valid() {
            assert!(check_state_transition(State::Unspent, State::Spent).is_ok());
        }

        #[test]
        fn pending_to_unspent_is_valid() {
            assert!(check_state_transition(State::Pending, State::Unspent).is_ok());
        }

        #[test]
        fn pending_to_spent_is_valid() {
            assert!(check_state_transition(State::Pending, State::Spent).is_ok());
        }

        #[test]
        fn unspent_to_unspent_is_invalid() {
            let result = check_state_transition(State::Unspent, State::Unspent);
            assert!(matches!(result, Err(Error::InvalidTransition(_, _))));
        }

        #[test]
        fn pending_to_pending_returns_pending_error() {
            let result = check_state_transition(State::Pending, State::Pending);
            assert!(matches!(result, Err(Error::Pending)));
        }

        #[test]
        fn spent_to_any_returns_already_spent() {
            assert!(matches!(
                check_state_transition(State::Spent, State::Unspent),
                Err(Error::AlreadySpent)
            ));
            assert!(matches!(
                check_state_transition(State::Spent, State::Pending),
                Err(Error::AlreadySpent)
            ));
            assert!(matches!(
                check_state_transition(State::Spent, State::Spent),
                Err(Error::AlreadySpent)
            ));
        }

        #[test]
        fn reserved_state_is_invalid_source() {
            let result = check_state_transition(State::Reserved, State::Unspent);
            assert!(matches!(result, Err(Error::InvalidTransition(_, _))));
        }
    }

    mod melt_quote_state_transitions {
        use super::*;

        #[test]
        fn unpaid_to_pending_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unpaid,
                MeltQuoteState::Pending
            )
            .is_ok());
        }

        #[test]
        fn unpaid_to_failed_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unpaid,
                MeltQuoteState::Failed
            )
            .is_ok());
        }

        #[test]
        fn pending_to_unpaid_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Pending,
                MeltQuoteState::Unpaid
            )
            .is_ok());
        }

        #[test]
        fn pending_to_paid_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Pending,
                MeltQuoteState::Paid
            )
            .is_ok());
        }

        #[test]
        fn pending_to_failed_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Pending,
                MeltQuoteState::Failed
            )
            .is_ok());
        }

        #[test]
        fn failed_to_pending_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Failed,
                MeltQuoteState::Pending
            )
            .is_ok());
        }

        #[test]
        fn failed_to_unpaid_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Failed,
                MeltQuoteState::Unpaid
            )
            .is_ok());
        }

        #[test]
        fn unknown_to_any_is_valid() {
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unknown,
                MeltQuoteState::Unpaid
            )
            .is_ok());
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unknown,
                MeltQuoteState::Pending
            )
            .is_ok());
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unknown,
                MeltQuoteState::Paid
            )
            .is_ok());
            assert!(check_melt_quote_state_transition(
                MeltQuoteState::Unknown,
                MeltQuoteState::Failed
            )
            .is_ok());
        }

        #[test]
        fn unpaid_to_paid_is_invalid() {
            let result =
                check_melt_quote_state_transition(MeltQuoteState::Unpaid, MeltQuoteState::Paid);
            assert!(matches!(
                result,
                Err(Error::InvalidMeltQuoteTransition(_, _))
            ));
        }

        #[test]
        fn unpaid_to_unpaid_is_invalid() {
            let result =
                check_melt_quote_state_transition(MeltQuoteState::Unpaid, MeltQuoteState::Unpaid);
            assert!(matches!(
                result,
                Err(Error::InvalidMeltQuoteTransition(_, _))
            ));
        }

        #[test]
        fn pending_to_pending_returns_pending_error() {
            let result =
                check_melt_quote_state_transition(MeltQuoteState::Pending, MeltQuoteState::Pending);
            assert!(matches!(result, Err(Error::Pending)));
        }

        #[test]
        fn paid_to_any_returns_already_paid() {
            assert!(matches!(
                check_melt_quote_state_transition(MeltQuoteState::Paid, MeltQuoteState::Unpaid),
                Err(Error::AlreadyPaid)
            ));
            assert!(matches!(
                check_melt_quote_state_transition(MeltQuoteState::Paid, MeltQuoteState::Pending),
                Err(Error::AlreadyPaid)
            ));
            assert!(matches!(
                check_melt_quote_state_transition(MeltQuoteState::Paid, MeltQuoteState::Paid),
                Err(Error::AlreadyPaid)
            ));
            assert!(matches!(
                check_melt_quote_state_transition(MeltQuoteState::Paid, MeltQuoteState::Failed),
                Err(Error::AlreadyPaid)
            ));
        }

        #[test]
        fn failed_to_paid_is_invalid() {
            let result =
                check_melt_quote_state_transition(MeltQuoteState::Failed, MeltQuoteState::Paid);
            assert!(matches!(
                result,
                Err(Error::InvalidMeltQuoteTransition(_, _))
            ));
        }

        #[test]
        fn failed_to_failed_is_invalid() {
            let result =
                check_melt_quote_state_transition(MeltQuoteState::Failed, MeltQuoteState::Failed);
            assert!(matches!(
                result,
                Err(Error::InvalidMeltQuoteTransition(_, _))
            ));
        }
    }
}
