// Copyright (c) 2022-2023 Yuki Kishimoto
// Distributed under the MIT software license

//! Url

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::{ParseError, Url};

use crate::ensure_cdk;

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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MintUrl(String);

impl Serialize for MintUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Use the to_string implementation to get the correctly formatted URL
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MintUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as a string and then use from_str to parse it correctly
        let s = String::deserialize(deserializer)?;
        MintUrl::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl MintUrl {
    fn format_url(url: &str) -> Result<String, Error> {
        ensure_cdk!(!url.is_empty(), Error::InvalidUrl);

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
        let mut formatted_url = format!("{protocol}://{host}");
        if !path.is_empty() {
            formatted_url.push_str(&format!("/{}", path));
        }
        Ok(formatted_url)
    }

    /// Join onto url
    pub fn join(&self, path: &str) -> Result<Url, Error> {
        let url = Url::parse(&self.0)?;

        // Get the current path segments
        let base_path = url.path();

        // Check if the path has a trailing slash to avoid double slashes
        let normalized_path = if base_path.ends_with('/') {
            format!("{}{}", base_path, path)
        } else {
            format!("{}/{}", base_path, path)
        };

        // Create a new URL with the combined path
        let mut result = url.clone();
        result.set_path(&normalized_path);
        Ok(result)
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
    use crate::Token;

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

    #[test]
    fn test_join_paths() {
        let url_no_path = "http://url-to-check.com";

        let url = MintUrl::from_str(url_no_path).unwrap();
        assert_eq!(
            format!("{url_no_path}/hello/world"),
            url.join_paths(&["hello", "world"]).unwrap().to_string()
        );

        let url_no_path_with_slash = "http://url-to-check.com/";

        let url = MintUrl::from_str(url_no_path_with_slash).unwrap();
        assert_eq!(
            format!("{url_no_path_with_slash}hello/world"),
            url.join_paths(&["hello", "world"]).unwrap().to_string()
        );

        let url_with_path = "http://url-to-check.com/my/path";

        let url = MintUrl::from_str(url_with_path).unwrap();
        assert_eq!(
            format!("{url_with_path}/hello/world"),
            url.join_paths(&["hello", "world"]).unwrap().to_string()
        );

        let url_with_path_with_slash = "http://url-to-check.com/my/path/";

        let url = MintUrl::from_str(url_with_path_with_slash).unwrap();
        assert_eq!(
            format!("{url_with_path_with_slash}hello/world"),
            url.join_paths(&["hello", "world"]).unwrap().to_string()
        );
    }

    #[test]
    fn test_mint_url_slash_eqality() {
        let mint_url_with_slash_str = "https://mint.minibits.cash/Bitcoin/";
        let mint_url_with_slash = MintUrl::from_str(mint_url_with_slash_str).unwrap();

        let mint_url_without_slash_str = "https://mint.minibits.cash/Bitcoin";
        let mint_url_without_slash = MintUrl::from_str(mint_url_without_slash_str).unwrap();

        assert_eq!(mint_url_with_slash, mint_url_without_slash);
        assert_eq!(
            mint_url_with_slash.to_string(),
            mint_url_without_slash_str.to_string()
        );
    }

    #[test]
    fn test_token_equality_trailing_slash() {
        let token_with_slash = Token::from_str("cashuBo2FteCNodHRwczovL21pbnQubWluaWJpdHMuY2FzaC9CaXRjb2luL2F1Y3NhdGF0gaJhaUgAUAVQ8ElBRmFwgqRhYQhhc3hAYzg2NTZhZDg4MzVmOWVmMzVkYWQ1MTZjNGU5ZTU5ZjA3YzFmODg0NTc2NWY3M2FhNWMyMjVhOGI4MGM0ZGM0ZmFjWCECNpnvLdFcsaVbCPUlOzr78XtBoD3mm3jQcldsQ6iKUBFhZKNhZVggrER4tfjjiH0e-lf9H---us1yjQQi__ZCFB9yFwH4jDphc1ggZfP2KcQOWA110vLz11caZF1PuXN606caPO2ZCAhfdvphclggadgz0psQELNif3xJ5J2d_TJWtRKfDFSj7h2ZD4WSFeykYWECYXN4QGZlNjAzNjA1NWM1MzVlZTBlYjI3MjQ1NmUzNjJlNmNkOWViNDNkMWQxODg0M2MzMDQ4MGU0YzE2YjI0MDY5MDZhY1ghAilA3g2_NriE94uTPISd2CM-90x53mK5QNM2iyTFDlnTYWSjYWVYIExR7bUzqM6-lRU7PbbEfnPW1vnSzCEN4SArmJZqp_7bYXNYIJMKRTSlXumUjPWXX5V8-hGPSZ-OXZJiEWm6_IB93OUDYXJYIB8YsigK7dMX59Oiy4Rh05xU0n0rVAPV7g_YFx564ZVa").unwrap();

        let token_without_slash = Token::from_str("cashuBo2FteCJodHRwczovL21pbnQubWluaWJpdHMuY2FzaC9CaXRjb2luYXVjc2F0YXSBomFpSABQBVDwSUFGYXCCpGFhCGFzeEBjODY1NmFkODgzNWY5ZWYzNWRhZDUxNmM0ZTllNTlmMDdjMWY4ODQ1NzY1ZjczYWE1YzIyNWE4YjgwYzRkYzRmYWNYIQI2me8t0VyxpVsI9SU7Ovvxe0GgPeabeNByV2xDqIpQEWFko2FlWCCsRHi1-OOIfR76V_0f7766zXKNBCL_9kIUH3IXAfiMOmFzWCBl8_YpxA5YDXXS8vPXVxpkXU-5c3rTpxo87ZkICF92-mFyWCBp2DPSmxAQs2J_fEnknZ39Mla1Ep8MVKPuHZkPhZIV7KRhYQJhc3hAZmU2MDM2MDU1YzUzNWVlMGViMjcyNDU2ZTM2MmU2Y2Q5ZWI0M2QxZDE4ODQzYzMwNDgwZTRjMTZiMjQwNjkwNmFjWCECKUDeDb82uIT3i5M8hJ3YIz73THneYrlA0zaLJMUOWdNhZKNhZVggTFHttTOozr6VFTs9tsR-c9bW-dLMIQ3hICuYlmqn_tthc1ggkwpFNKVe6ZSM9ZdflXz6EY9Jn45dkmIRabr8gH3c5QNhclggHxiyKArt0xfn06LLhGHTnFTSfStUA9XuD9gXHnrhlVo").unwrap();

        let url_with_slash = token_with_slash.mint_url().unwrap();
        let url_without_slash = token_without_slash.mint_url().unwrap();

        assert_eq!(url_without_slash.to_string(), url_with_slash.to_string());
        assert_eq!(url_without_slash, url_with_slash);
    }
}
