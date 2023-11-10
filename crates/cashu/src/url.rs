// Copyright (c) 2022-2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Url

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{ParseError, Url};

/// Url Error
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Url error
    #[error("`{0}`")]
    Url(#[from] ParseError),
}

/// Unchecked Url
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UncheckedUrl(String);

impl UncheckedUrl {
    /// New unchecked url
    pub fn new<S>(url: S) -> Self
    where
        S: Into<String>,
    {
        Self(url.into())
    }

    /// Empty unchecked url
    pub fn empty() -> Self {
        Self(String::new())
    }

    pub fn join(&self, path: &str) -> Result<Url, Error> {
        let url: Url = self.try_into()?;
        Ok(url.join(path)?)
    }
}

impl<S> From<S> for UncheckedUrl
where
    S: Into<String>,
{
    fn from(url: S) -> Self {
        Self(url.into())
    }
}

impl FromStr for UncheckedUrl {
    type Err = Error;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(url))
    }
}

impl TryFrom<UncheckedUrl> for Url {
    type Error = Error;

    fn try_from(unchecked_url: UncheckedUrl) -> Result<Url, Self::Error> {
        Ok(Self::parse(&unchecked_url.0)?)
    }
}

impl TryFrom<&UncheckedUrl> for Url {
    type Error = Error;

    fn try_from(unchecked_url: &UncheckedUrl) -> Result<Url, Self::Error> {
        Ok(Self::parse(unchecked_url.0.as_str())?)
    }
}

impl fmt::Display for UncheckedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_unchecked_relay_url() {
        let relay = "wss://relay.damus.io:8333/";

        let unchecked_relay_url = UncheckedUrl::from_str(relay).unwrap();

        assert_eq!(relay, unchecked_relay_url.to_string());

        // assert_eq!(relay, serde_json::to_string(&unchecked_relay_url).unwrap());

        let relay = "wss://relay.damus.io:8333";

        let unchecked_relay_url = UncheckedUrl::from_str(relay).unwrap();

        assert_eq!(relay, unchecked_relay_url.to_string());

        // assert_eq!(relay,
        // serde_json::to_string(&unchecked_relay_url).unwrap())
    }
}
