//! Shared outbound transport for federation runtimes.

use cdk_http_client::Async;

/// Concrete hybrid transport shared by every federation worker in one mint runtime.
#[cfg(feature = "iroh")]
pub type FederationTransport = cdk_iroh::IrohTransport<Async>;

/// Concrete HTTP transport shared by every federation worker in one mint runtime.
#[cfg(not(feature = "iroh"))]
pub type FederationTransport = Async;

/// Build the default federation transport.
#[must_use]
pub fn default_federation_transport() -> FederationTransport {
    FederationTransport::default()
}

/// Build a hybrid federation transport backed by the mint-owned Iroh node.
#[cfg(feature = "iroh")]
#[must_use]
pub fn federation_transport_with_iroh_node(node: cdk_iroh::IrohNode) -> FederationTransport {
    FederationTransport::with_node(node, Async::default())
}
