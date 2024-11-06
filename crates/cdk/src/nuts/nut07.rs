//! NUT-07: Spendable Check
//!
//! <https://github.com/cashubtc/nuts/blob/main/07.md>

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut01::PublicKey;

/// NUT07 Error
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Unknown State error
    #[error("Unknown state")]
    UnknownState,
}

/// State of Proof
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum State {
    /// Spent
    Spent,
    /// Unspent
    Unspent,
    /// Pending
    ///
    /// Currently being used in a transaction i.e. melt in progress
    Pending,
    /// Proof is reserved
    ///
    /// i.e. used to create a token
    Reserved,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Spent => "SPENT",
            Self::Unspent => "UNSPENT",
            Self::Pending => "PENDING",
            Self::Reserved => "RESERVED",
        };

        write!(f, "{}", s)
    }
}

impl FromStr for State {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state {
            "SPENT" => Ok(Self::Spent),
            "UNSPENT" => Ok(Self::Unspent),
            "PENDING" => Ok(Self::Pending),
            "RESERVED" => Ok(Self::Reserved),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Check spendable request [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct CheckStateRequest {
    /// Y's of the proofs to check
    #[serde(rename = "Ys")]
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<String>, max_items = 1_000))]
    pub ys: Vec<PublicKey>,
}

/// Proof state [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct ProofState {
    /// Y of proof
    #[serde(rename = "Y")]
    #[cfg_attr(feature = "swagger", schema(value_type = String))]
    pub y: PublicKey,
    /// State of proof
    pub state: State,
    /// Witness data if it is supplied
    pub witness: Option<String>,
}

impl From<(PublicKey, State)> for ProofState {
    fn from(value: (PublicKey, State)) -> Self {
        Self {
            y: value.0,
            state: value.1,
            witness: None,
        }
    }
}

/// Check Spendable Response [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct CheckStateResponse {
    /// Proof states
    pub states: Vec<ProofState>,
}
