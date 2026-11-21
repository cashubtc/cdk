//! NUT-07: Spendable Check
//!
//! <https://github.com/cashubtc/nuts/blob/main/07.md>

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut01::ProofId;
use super::nut02::KeySetVersion;
use super::Witness;

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
pub enum State {
    /// Spent
    Spent,
    /// Unspent
    Unspent,
    /// Pending
    ///
    /// Currently being used in a transaction i.e. melt in progress
    Pending,
    /// Reserved
    ///
    /// Proof is reserved for future token creation
    Reserved,
    /// Pending spent (i.e., spent but not yet swapped by receiver)
    PendingSpent,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Spent => "SPENT",
            Self::Unspent => "UNSPENT",
            Self::Pending => "PENDING",
            Self::Reserved => "RESERVED",
            Self::PendingSpent => "PENDING_SPENT",
        };

        write!(f, "{s}")
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
            "PENDING_SPENT" => Ok(Self::PendingSpent),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Check spendable request [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStateRequest {
    /// Y's of the proofs to check
    #[serde(rename = "Ys")]
    pub ys: Vec<ProofId>,
}

/// Proof state [NUT-07]
///
/// A v3 proof's state (its `Y` is a 48-byte G1 point) also carries the
/// `input_digest` its witness signed, both present while it is pending or
/// spent. Pre-v3 states serialize exactly as before, without the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofState {
    /// Y of proof
    pub y: ProofId,
    /// State of proof
    pub state: State,
    /// Witness data if it is supplied
    pub witness: Option<Witness>,
    /// Input digest the witness signed (v3 only)
    pub input_digest: Option<[u8; 32]>,
}

impl ProofState {
    /// Whether this is the state of a v3 proof
    pub fn is_v3(&self) -> bool {
        self.y.as_bytes().len() == 48
    }
}

impl From<(ProofId, State)> for ProofState {
    fn from(value: (ProofId, State)) -> Self {
        Self {
            y: value.0,
            state: value.1,
            witness: None,
            input_digest: None,
        }
    }
}

impl Serialize for ProofState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let v3 = self.is_v3();
        let mut state = serializer.serialize_struct("ProofState", 3 + usize::from(v3))?;
        state.serialize_field("Y", &self.y)?;
        state.serialize_field("state", &self.state)?;
        state.serialize_field("witness", &self.witness)?;
        if v3 {
            state.serialize_field(
                "input_digest",
                &self.input_digest.map(crate::util::hex::encode),
            )?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for ProofState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        #[derive(Deserialize)]
        struct Encoded {
            #[serde(rename = "Y")]
            y: ProofId,
            state: State,
            #[serde(default)]
            witness: Option<serde_json::Value>,
            #[serde(default)]
            input_digest: Option<String>,
        }

        let encoded = Encoded::deserialize(deserializer)?;
        let version = match encoded.y.as_bytes().len() {
            48 => KeySetVersion::Version02,
            _ => KeySetVersion::Version00,
        };
        let witness = encoded
            .witness
            .filter(|value| !value.is_null())
            .map(|value| Witness::from_value_for_version(value, version))
            .transpose()
            .map_err(D::Error::custom)?;
        let input_digest = encoded
            .input_digest
            .map(|hex| {
                crate::util::hex::decode(&hex)
                    .ok()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                    .ok_or_else(|| D::Error::custom("input_digest must be 32 bytes of hex"))
            })
            .transpose()?;
        Ok(Self {
            y: encoded.y,
            state: encoded.state,
            witness,
            input_digest,
        })
    }
}

/// Check Spendable Response [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStateResponse {
    /// Proof states
    pub states: Vec<ProofState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_display_and_from_str_cover_all_protocol_values() {
        let cases = [
            (State::Spent, "SPENT"),
            (State::Unspent, "UNSPENT"),
            (State::Pending, "PENDING"),
            (State::Reserved, "RESERVED"),
            (State::PendingSpent, "PENDING_SPENT"),
        ];

        for (state, wire) in cases {
            assert_eq!(state.to_string(), wire);
            assert_eq!(State::from_str(wire), Ok(state));
        }

        assert_eq!(State::from_str("UNKNOWN"), Err(Error::UnknownState));
    }
}
