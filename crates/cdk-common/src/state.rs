//! State transition rules

use cashu::State;

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
}

#[inline]
/// Check if the state transition is allowed
pub fn check_state_transition(current_state: State, new_state: State) -> Result<(), Error> {
    let is_valid_transition = match current_state {
        State::Unspent => matches!(
            new_state,
            State::Pending | State::Reserved | State::PendingSpent | State::Spent
        ),
        State::Pending => matches!(new_state, State::Unspent | State::Spent),
        State::Reserved => matches!(new_state, State::Pending | State::Unspent),
        State::PendingSpent => matches!(new_state, State::Unspent | State::Spent),
        State::Spent => false,
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
