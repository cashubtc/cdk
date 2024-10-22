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
    #[error(transparent)]
    Url(#[from] ParseError),
    /// Invalid URL structure
    #[error("Invalid URL")]
    InvalidUrl,
}

/// MintUrl Url
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MintUrl(String);

impl MintUrl {
    fn format_url(url: &str) -> Result<String, Error> {
        if url.is_empty() {
            return Err(Error::InvalidUrl);
        }
        let url = url.trim_end_matches('/');
        // https://URL.com/path/TO/resource -> https://url.com/path/TO/resource
        let protocol = url
            .split("://")
            .nth(0)
            .ok_or(Error::InvalidUrl)?
            .to_lowercase();
        let host = url
            .split("://")
            .nth(1)
            .ok_or(Error::InvalidUrl)?
            .split('/')
            .nth(0)
            .ok_or(Error::InvalidUrl)?
            .to_lowercase();
        let path = url
            .split("://")
            .nth(1)
            .ok_or(Error::InvalidUrl)?
            .split('/')
            .skip(1)
            .collect::<Vec<&str>>()
            .join("/");
        let mut formatted_url = format!("{}://{}", protocol, host);
        if !path.is_empty() {
            formatted_url.push_str(&format!("/{}", path));
        }
        Ok(formatted_url)
    }

    /// Join onto url
    pub fn join(&self, path: &str) -> Result<Url, Error> {
        Url::parse(&self.0)
            .and_then(|url| url.join(path))
            .map_err(Into::into)
    }

    /// Append path elements onto the URL
    pub fn join_paths(&self, path_elements: &[&str]) -> Result<Url, Error> {
        self.join(&path_elements.join("/"))
    }
}

impl FromStr for MintUrl {
    type Err = Error;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        let formatted_url = Self::format_url(url);
        match formatted_url {
            Ok(url) => Ok(Self(url)),
            Err(_) => Err(Error::InvalidUrl),
        }
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
        assert_eq!(formatted_url, very_trimmed_url.to_string());

        let trimmed_url = MintUrl::from_str(unformatted_url).unwrap();
        assert_eq!(formatted_url, trimmed_url.to_string());

        let unchanged_url = MintUrl::from_str(formatted_url).unwrap();
        assert_eq!(formatted_url, unchanged_url.to_string());
    }
    #[test]
    fn test_case_insensitive() {
        let wrong_cased_url = "http://URL-to-check.com";
        let correct_cased_url = "http://url-to-check.com";

        let cased_url_formatted = MintUrl::from_str(wrong_cased_url).unwrap();
        assert_eq!(correct_cased_url, cased_url_formatted.to_string());

        let wrong_cased_url_with_path = "http://URL-to-check.com/PATH/to/check";
        let correct_cased_url_with_path = "http://url-to-check.com/PATH/to/check";

        let cased_url_with_path_formatted = MintUrl::from_str(wrong_cased_url_with_path).unwrap();
        assert_eq!(
            correct_cased_url_with_path,
            cased_url_with_path_formatted.to_string()
        );
    }
}
