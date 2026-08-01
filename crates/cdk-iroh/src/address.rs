//! Strict parsing for endpoint-ID Iroh URLs.

use std::str::FromStr;

use iroh::EndpointId;
use url::Url;

use crate::Error;

#[derive(Debug, Clone)]
pub(crate) struct IrohTarget {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) authority: String,
    pub(crate) request_target: String,
}

impl IrohTarget {
    pub(crate) fn parse(url: &Url) -> Result<Self, Error> {
        if url.scheme() != "iroh" {
            return Err(Error::UnsupportedScheme {
                scheme: url.scheme().to_owned(),
            });
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::InvalidUrl {
                reason: "userinfo is forbidden",
            });
        }
        if url.port().is_some() {
            return Err(Error::InvalidUrl {
                reason: "ports are forbidden",
            });
        }
        if url.fragment().is_some() {
            return Err(Error::InvalidUrl {
                reason: "fragments are forbidden",
            });
        }
        let authority = url.host_str().ok_or(Error::InvalidUrl {
            reason: "endpoint ID is required",
        })?;
        let endpoint_id = EndpointId::from_str(authority).map_err(|_| Error::InvalidUrl {
            reason: "host is not an endpoint ID",
        })?;
        let mut request_target = match url.path() {
            "" => "/".to_string(),
            path => path.to_owned(),
        };
        if let Some(query) = url.query() {
            request_target.push('?');
            request_target.push_str(query);
        }
        Ok(Self {
            endpoint_id,
            authority: endpoint_id.to_string(),
            request_target,
        })
    }
}

pub(crate) fn peer_fingerprint(endpoint_id: EndpointId) -> String {
    endpoint_id.fmt_short().to_string()
}
