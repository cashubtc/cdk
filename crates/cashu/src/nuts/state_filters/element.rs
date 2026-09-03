//! Filter elements.
//!
//! One element per object whose observable state changed during an epoch.

use std::fmt;
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash};
use serde::{Deserialize, Serialize};

use super::Error;
use crate::nuts::{MeltQuoteState, PublicKey, State};

/// Domain separator, hashed as raw ASCII bytes without a length prefix.
pub const DOMAIN_SEPARATOR: &[u8] = b"Cashu_StateFilter_v1";

/// The kind of object an element describes.
///
/// A kind names the operation, not the payment method. Every method shares one
/// filter sequence; splitting it would shrink the anonymity set and leak the
/// method on a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    /// A proof changed state.
    ProofState,
    /// A mint quote changed.
    MintQuote,
    /// A melt quote changed state.
    MeltQuote,
}

impl FilterKind {
    /// The kind as it appears in an element preimage and on the wire.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProofState => "proof_state",
            Self::MintQuote => "mint_quote",
            Self::MeltQuote => "melt_quote",
        }
    }
}

impl fmt::Display for FilterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for FilterKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proof_state" => Ok(Self::ProofState),
            "mint_quote" => Ok(Self::MintQuote),
            "melt_quote" => Ok(Self::MeltQuote),
            other => Err(Error::UnknownKind(other.to_string())),
        }
    }
}

/// A single filter element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FilterElement([u8; 32]);

fn len32(value: usize) -> [u8; 4] {
    (value as u32).to_be_bytes()
}

impl FilterElement {
    /// Hash a `(kind, id, state)` triple into an element.
    ///
    /// The length prefixes keep the three fields unambiguous, so a new kind
    /// needs no registry of tags.
    pub fn new(kind: FilterKind, id: &[u8], state: &[u8]) -> Self {
        let kind = kind.as_str().as_bytes();

        let mut preimage =
            Vec::with_capacity(DOMAIN_SEPARATOR.len() + 12 + kind.len() + id.len() + state.len());
        preimage.extend_from_slice(DOMAIN_SEPARATOR);
        preimage.extend_from_slice(&len32(kind.len()));
        preimage.extend_from_slice(kind);
        preimage.extend_from_slice(&len32(id.len()));
        preimage.extend_from_slice(id);
        preimage.extend_from_slice(&len32(state.len()));
        preimage.extend_from_slice(state);

        Self(sha256::Hash::hash(&preimage).to_byte_array())
    }

    /// Element for a proof that moved to `state`.
    ///
    /// Binding the state into the element is what keeps a proof state private
    /// end to end: a match reports the new state directly, so the wallet never
    /// has to send `Y` to learn it.
    pub fn proof_state(y: &PublicKey, state: State) -> Result<Self, Error> {
        let state = match state {
            State::Unspent => "UNSPENT",
            State::Pending => "PENDING",
            State::Spent => "SPENT",
            other => return Err(Error::UnsupportedState(other.to_string())),
        };

        Ok(Self::new(
            FilterKind::ProofState,
            &y.to_bytes(),
            state.as_bytes(),
        ))
    }

    /// Element for a mint quote that changed.
    ///
    /// Mint quotes carry no state because their accounting fields would
    /// disclose amounts, so a match means only that the quote changed.
    ///
    /// `id` must be the quote id exactly as it was returned to the wallet,
    /// neither case-normalized nor stripped of hyphens.
    pub fn mint_quote(id: &str) -> Self {
        Self::new(FilterKind::MintQuote, id.as_bytes(), b"")
    }

    /// Element for a melt quote that moved to `state`.
    ///
    /// A failed melt is published as `UNPAID`: the spec's melt enum has three
    /// values and a fourth string would break interop, and a wallet that
    /// matches learns the real state from the quote endpoint.
    pub fn melt_quote(id: &str, state: MeltQuoteState) -> Result<Self, Error> {
        let state = match state {
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => "UNPAID",
            MeltQuoteState::Pending => "PENDING",
            MeltQuoteState::Paid => "PAID",
            other => return Err(Error::UnsupportedState(other.to_string())),
        };

        Ok(Self::new(
            FilterKind::MeltQuote,
            id.as_bytes(),
            state.as_bytes(),
        ))
    }

    /// The element digest.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuild an element from a stored digest.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for FilterElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", crate::util::hex::encode(self.0))
    }
}
