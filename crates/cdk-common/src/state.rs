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
