//! Compact state filters
//!
//! Filters over the mint's state changes that the mint publishes for everyone
//! and that wallets match locally, so a wallet learns that its ecash was spent,
//! or its quote paid, without telling the mint which.

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

mod codec;
mod element;

pub use codec::{encode, DecodedFilter, MAX_P, MIN_P};
pub use element::{FilterElement, FilterKind, DOMAIN_SEPARATOR};

/// Recommended Golomb-Rice parameter.
pub const DEFAULT_P: u8 = 28;

/// Recommended epoch duration in seconds.
pub const DEFAULT_EPOCH: u64 = 3600;

/// Recommended number of filters on a full page.
pub const DEFAULT_PAGE_SIZE: u64 = 50;

/// Smallest Golomb-Rice parameter a mint should publish with.
///
/// Below this, false positives stop being rare across the whole history a
/// wallet tests, even though they stay rare within any one filter.
pub const RECOMMENDED_MIN_P: u8 = 24;

/// State filter error
#[derive(Debug, ThisError, PartialEq, Eq)]
pub enum Error {
    /// Golomb-Rice parameter outside the range this implementation supports
    #[error("Golomb-Rice parameter {0} out of range, must be between 7 and 63")]
    InvalidParameter(u8),
    /// Filter data ran out in the middle of a code
    #[error("Filter data ended in the middle of a code")]
    UnexpectedEnd,
    /// Filter data encodes a position that does not fit in 64 bits
    #[error("Filter data encodes an out of range position")]
    PositionOverflow,
    /// State has no representation in a filter element
    #[error("State {0} cannot appear in a state filter")]
    UnsupportedState(String),
    /// Unrecognized filter kind
    #[error("Unknown filter kind: {0}")]
    UnknownKind(String),
    /// Filter data was not valid hex
    #[error("Invalid filter hex: {0}")]
    Hex(String),
}

/// A filter covering one epoch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Filter {
    /// Unix timestamp at which the epoch began
    pub start: u64,
    /// Unix timestamp at which the epoch ended
    pub end: u64,
    /// Hex encoded Golomb-Rice coded set
    pub data: String,
}

impl Filter {
    /// Decode the coded set.
    pub fn decode(&self, p: u8) -> Result<DecodedFilter, Error> {
        let data = crate::util::hex::decode(&self.data).map_err(|e| Error::Hex(e.to_string()))?;
        DecodedFilter::decode(&data, p)
    }
}

/// The filter of the epoch that is still open.
///
/// It has no `end` because its epoch has not closed, and the closed filter
/// carrying the same `start` supersedes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingFilterResponse {
    /// Unix timestamp at which the open epoch began
    pub start: u64,
    /// Hex encoded Golomb-Rice coded set
    pub data: String,
}

impl PendingFilterResponse {
    /// Decode the coded set.
    ///
    /// `data` changes between requests, and so does the `n` decoded from it,
    /// so callers must recompute every candidate position per response.
    pub fn decode(&self, p: u8) -> Result<DecodedFilter, Error> {
        let data = crate::util::hex::decode(&self.data).map_err(|e| Error::Hex(e.to_string()))?;
        DecodedFilter::decode(&data, p)
    }
}

/// Parameters of a mint's filters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetFiltersInfoResponse {
    /// Golomb-Rice parameter
    pub p: u8,
    /// Epoch duration in seconds
    pub epoch: u64,
    /// Kinds covered by the filters
    pub kinds: Vec<FilterKind>,
    /// Number of filters on a full page
    pub page_size: u64,
    /// Lowest page number the mint still serves
    pub first_page: u64,
    /// Page still being filled; every page below it is complete
    pub current_page: u64,
    /// How many filters the current page holds so far
    pub current_page_count: u64,
    /// Start of the oldest filter the mint still serves
    pub earliest_start: u64,
    /// End of the most recent filter
    pub latest_end: u64,
    /// Whether the mint serves the pending filter
    pub pending: bool,
}

/// A page of filters, in ascending order of `start`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetFiltersResponse {
    /// Page number
    pub page: u64,
    /// The filters on this page
    pub filters: Vec<Filter>,
}

/// Mint info settings for compact state filters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Whether the mint publishes filters
    pub supported: bool,
    /// Kinds the mint's filters cover
    ///
    /// A mint that advertises a kind covers it for every payment method it
    /// supports. Partial coverage is not expressible, because a match would
    /// then disclose the method.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<FilterKind>,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(kinds: Vec<FilterKind>) -> Self {
        Self {
            supported: true,
            kinds,
        }
    }

    /// Whether these settings carry nothing worth serializing
    pub fn is_empty(&self) -> bool {
        !self.supported && self.kinds.is_empty()
    }
}

#[cfg(test)]
mod tests;
