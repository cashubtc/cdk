//! Android DNS integration that does not require unsafe JNI bootstrap code.

use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::pin::Pin;

use iroh::dns::{BoxIter, DnsError, DnsResolver, Resolver, TxtRecordData};

/// Resolve host addresses through Android's native `getaddrinfo` implementation.
///
/// Iroh's default resolver reads Android network state through `ndk_context`,
/// which requires unsafe JNI initialization. CDK forbids unsafe code, so the
/// Android build uses Tokio's system resolver for address records instead.
/// N0's HTTPS pkarr lookup only needs these address records; the parallel DNS
/// TXT lookup is allowed to fail and the remaining discovery mechanisms
/// continue normally.
#[derive(Debug, Clone, Copy, Default)]
struct AndroidSystemDnsResolver;

impl Resolver for AndroidSystemDnsResolver {
    fn lookup_ipv4(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = Result<BoxIter<Ipv4Addr>, DnsError>> + Send + 'static>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|_| DnsError::NoResponse {
                    meta: Default::default(),
                })?;
            let addresses = addresses
                .filter_map(|address| match address.ip() {
                    std::net::IpAddr::V4(address) => Some(address),
                    std::net::IpAddr::V6(_) => None,
                })
                .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err(DnsError::NoResponse {
                    meta: Default::default(),
                });
            }
            Ok(Box::new(addresses.into_iter()) as BoxIter<Ipv4Addr>)
        })
    }

    fn lookup_ipv6(
        &self,
        host: String,
    ) -> Pin<Box<dyn Future<Output = Result<BoxIter<Ipv6Addr>, DnsError>> + Send + 'static>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|_| DnsError::NoResponse {
                    meta: Default::default(),
                })?;
            let addresses = addresses
                .filter_map(|address| match address.ip() {
                    std::net::IpAddr::V4(_) => None,
                    std::net::IpAddr::V6(address) => Some(address),
                })
                .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err(DnsError::NoResponse {
                    meta: Default::default(),
                });
            }
            Ok(Box::new(addresses.into_iter()) as BoxIter<Ipv6Addr>)
        })
    }

    fn lookup_txt(
        &self,
        _host: String,
    ) -> Pin<Box<dyn Future<Output = Result<BoxIter<TxtRecordData>, DnsError>> + Send + 'static>>
    {
        Box::pin(async {
            Err(DnsError::NoResponse {
                meta: Default::default(),
            })
        })
    }

    fn clear_cache(&self) {}

    fn reset(&self) -> Box<dyn Resolver> {
        Box::new(Self)
    }
}

pub(crate) fn system_dns_resolver() -> DnsResolver {
    DnsResolver::custom(AndroidSystemDnsResolver)
}
