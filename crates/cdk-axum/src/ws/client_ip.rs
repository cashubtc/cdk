//! Resolving the address the per-address WebSocket limit is keyed on.

use std::net::{IpAddr, SocketAddr};

use axum::http::{HeaderMap, HeaderName};

/// The address to hold to the per-address connection limit.
///
/// With no `header` configured this is the TCP peer, which is the only address
/// nothing but the network can forge. A configured `header` is read instead,
/// because behind a reverse proxy the peer is the proxy and every client would
/// otherwise share one allowance.
///
/// Only the rightmost entry of the header is read: a proxy appends the address
/// it saw to whatever the client sent, so anything forged sits to its left. A
/// header that is absent or does not end in an address falls back to the peer,
/// which is what a request that skipped the proxy looks like.
pub(crate) fn client_ip(
    header: Option<&HeaderName>,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Option<IpAddr> {
    header
        .and_then(|name| forwarded_ip(name, headers))
        .or_else(|| peer.map(|addr| addr.ip()))
}

/// The last address the given header carries, across repeated header lines.
fn forwarded_ip(name: &HeaderName, headers: &HeaderMap) -> Option<IpAddr> {
    let last = headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .rfind(|value| !value.trim().is_empty())?;

    parse_entry(last.rsplit(',').next()?)
}

/// One forwarded entry, which may carry a port and may be a bracketed IPv6
/// address, as `X-Forwarded-For` and the headers modelled on it all do.
fn parse_entry(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    let unbracketed = entry
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(entry);

    unbracketed
        .parse::<IpAddr>()
        .ok()
        .or_else(|| entry.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use axum::http::HeaderValue;

    use super::*;

    fn forwarded_for() -> HeaderName {
        HeaderName::from_static("x-forwarded-for")
    }

    fn peer() -> Option<SocketAddr> {
        Some(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), 4242)))
    }

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                forwarded_for(),
                HeaderValue::from_str(value).expect("test header value"),
            );
        }
        headers
    }

    fn resolved(values: &[&str]) -> Option<IpAddr> {
        client_ip(Some(&forwarded_for()), &headers(values), peer())
    }

    #[test]
    fn without_a_configured_header_the_peer_is_used() {
        assert_eq!(
            client_ip(None, &headers(&["203.0.113.7"]), peer()),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
            "an unconfigured header must not be trusted"
        );
    }

    #[test]
    fn a_single_entry_is_read() {
        assert_eq!(
            resolved(&["203.0.113.7"]),
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
        );
    }

    /// The proxy appends what it saw, so the rightmost entry is the only one it
    /// vouches for; everything left of it can be client-supplied.
    #[test]
    fn the_rightmost_entry_wins() {
        assert_eq!(
            resolved(&["1.1.1.1, 203.0.113.7"]),
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
        );
        assert_eq!(
            resolved(&["1.1.1.1", "203.0.113.7"]),
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))),
            "a repeated header line is a continuation of the same list"
        );
    }

    #[test]
    fn a_port_and_ipv6_brackets_are_accepted() {
        let expected = Some(IpAddr::V6(Ipv6Addr::LOCALHOST));

        assert_eq!(resolved(&["::1"]), expected);
        assert_eq!(resolved(&["[::1]"]), expected);
        assert_eq!(resolved(&["[::1]:8080"]), expected);
        assert_eq!(
            resolved(&["203.0.113.7:8080"]),
            Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)))
        );
    }

    /// A forged or missing header must not silently widen an allowance, and it
    /// must not be worked around by reading further left either.
    #[test]
    fn an_unusable_header_falls_back_to_the_peer() {
        let fallback = Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));

        assert_eq!(resolved(&[]), fallback);
        assert_eq!(resolved(&[""]), fallback);
        assert_eq!(resolved(&["unknown"]), fallback);
        assert_eq!(resolved(&["203.0.113.7, unknown"]), fallback);
    }

    #[test]
    fn without_a_peer_an_unusable_header_yields_no_address() {
        assert_eq!(
            client_ip(Some(&forwarded_for()), &headers(&["unknown"]), None),
            None
        );
    }
}
