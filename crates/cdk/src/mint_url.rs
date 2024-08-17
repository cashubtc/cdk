// Copyright (c) 2022-2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Url

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use url::{ParseError, Url};

/// Url Error
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Url error
    #[error(transparent)]
    Url(#[from] ParseError),
}

/// MintUrl Url
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct MintUrl(String);

impl MintUrl {
    /// New mint url
    pub fn new<S>(url: S) -> Self
    where
        S: Into<String>,
    {
        let url: String = url.into();
        Self(url.trim_end_matches('/').to_string())
    }

    /// Empty mint url
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Join onto url
    pub fn join(&self, path: &str) -> Result<Url, Error> {
        let url: Url = self.try_into()?;
        Ok(url.join(path)?)
    }

    /// Remove trailing slashes from url
    pub fn trim_trailing_slashes(&self) -> Self {
        Self(self.to_string().trim_end_matches('/').to_string())
    }
}

impl<'de> Deserialize<'de> for MintUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        MintUrl::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl<S> From<S> for MintUrl
where
    S: Into<String>,
{
    fn from(url: S) -> Self {
        let url: String = url.into();
        Self(url.trim_end_matches('/').to_string())
    }
}

impl FromStr for MintUrl {
    type Err = Error;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(url))
    }
}

impl TryFrom<MintUrl> for Url {
    type Error = Error;

    fn try_from(mint_url: MintUrl) -> Result<Url, Self::Error> {
        Ok(Self::parse(&mint_url.0)?)
    }
}

impl TryFrom<&MintUrl> for Url {
    type Error = Error;

    fn try_from(mint_url: &MintUrl) -> Result<Url, Self::Error> {
        Ok(Self::parse(mint_url.0.as_str())?)
    }
}

impl fmt::Display for MintUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_trim_trailing_slashes() {
        let very_unformatted_url = "http://url-to-check.com////";
        let unformatted_url = "http://url-to-check.com/";
        let formatted_url = "http://url-to-check.com";

        let very_trimmed_url = MintUrl::from_str(very_unformatted_url).unwrap();
        assert_eq!("http://url-to-check.com", very_trimmed_url.to_string());

        let trimmed_url = MintUrl::from_str(unformatted_url).unwrap();
        assert_eq!("http://url-to-check.com", trimmed_url.to_string());

        let unchanged_url = MintUrl::from_str(formatted_url).unwrap();
        assert_eq!("http://url-to-check.com", unchanged_url.to_string());
    }
}
