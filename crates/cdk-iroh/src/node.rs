//! Endpoint identity, discovery, and lifecycle.

use std::{fmt, sync::Arc};

use iroh::{
    address_lookup::{memory::MemoryLookup, DnsAddressLookup, PkarrResolver},
    endpoint::{presets, Builder, QuicTransportConfig, VarInt},
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
};

use crate::{
    address::peer_fingerprint, config::DiscoveryMode, metrics::MetricsInner, pool::ConnectionPool,
    EndpointTicket, Error, IrohConfig, MetricsSnapshot,
};

#[derive(Debug)]
struct NodeInner {
    endpoint: Endpoint,
    lookup: MemoryLookup,
    config: IrohConfig,
    role: EndpointRole,
    pool: ConnectionPool,
    metrics: Arc<MetricsInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointRole {
    Client,
    Server,
}

/// A clone-shared Iroh endpoint with explicit identity and discovery lifecycle.
#[derive(Clone)]
pub struct IrohNode {
    inner: Arc<NodeInner>,
}

impl IrohNode {
    /// Creates a listener-capable endpoint with a newly generated ephemeral identity.
    pub async fn ephemeral(config: IrohConfig) -> Result<Self, Error> {
        Self::bind(config, None, EndpointRole::Server).await
    }

    /// Creates an outgoing-only endpoint with a newly generated ephemeral identity.
    ///
    /// N0 clients resolve mint addresses and may use relays for reachability, but
    /// do not publish their own address.
    pub async fn client(config: IrohConfig) -> Result<Self, Error> {
        Self::bind(config, None, EndpointRole::Client).await
    }

    /// Creates an endpoint with an operator-provided persistent identity.
    pub async fn persistent(config: IrohConfig, secret_key: SecretKey) -> Result<Self, Error> {
        Self::bind(config, Some(secret_key), EndpointRole::Server).await
    }

    async fn bind(
        config: IrohConfig,
        secret_key: Option<SecretKey>,
        role: EndpointRole,
    ) -> Result<Self, Error> {
        validate_config(&config)?;
        let lookup = MemoryLookup::new();
        for ticket in &config.static_tickets {
            lookup.add_endpoint_info(ticket.endpoint_addr().clone());
        }

        let mut builder = match (&config.discovery, role) {
            (DiscoveryMode::N0, EndpointRole::Server) => Endpoint::builder(presets::N0),
            (DiscoveryMode::N0, EndpointRole::Client) => Endpoint::builder(presets::Minimal)
                .relay_mode(iroh::endpoint::default_relay_mode())
                .clear_address_lookup()
                .address_lookup(PkarrResolver::n0_dns())
                .address_lookup(DnsAddressLookup::n0_dns()),
            (DiscoveryMode::Static, _) => Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .clear_address_lookup(),
            (DiscoveryMode::Custom { relay_urls }, _) => {
                Endpoint::builder(presets::Minimal)
                    .relay_mode(RelayMode::custom(relay_urls.clone()))
                    .clear_address_lookup()
            }
        };
        builder = builder.address_lookup(lookup.clone());
        builder = builder.alpns(vec![crate::protocol::ALPN.to_vec()]);
        let incoming_streams = match role {
            EndpointRole::Client => 0,
            EndpointRole::Server => u32::try_from(config.limits.max_streams_per_connection)
                .map_err(|_| Error::Endpoint)?,
        };
        let transport_config = QuicTransportConfig::builder()
            .max_concurrent_bidi_streams(VarInt::from_u32(incoming_streams))
            .max_concurrent_uni_streams(VarInt::from_u32(0))
            .build();
        builder = builder.transport_config(transport_config);
        if let Some(bind_addr) = config.bind_addr {
            builder = apply_bind_addr(builder, bind_addr)?;
        }
        if let Some(secret_key) = secret_key {
            builder = builder.secret_key(secret_key);
        }
        let endpoint = builder.bind().await.map_err(|_| Error::Endpoint)?;
        let metrics = Arc::new(MetricsInner::default());
        metrics.set_online(true);
        let pool = ConnectionPool::new(
            config.timeouts,
            config.limits.max_pooled_connections,
            metrics.clone(),
        );
        tracing::info!(peer = %endpoint.id().fmt_short(), discovery = ?config.discovery, "Iroh endpoint online");
        Ok(Self {
            inner: Arc::new(NodeInner {
                endpoint,
                lookup,
                config,
                role,
                pool,
                metrics,
            }),
        })
    }

    /// Returns the authenticated endpoint ID.
    pub fn endpoint_id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    /// Returns the currently advertised endpoint addresses.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.inner.endpoint.addr()
    }

    /// Returns a standard bootstrap ticket for this endpoint's current address.
    pub fn endpoint_ticket(&self) -> EndpointTicket {
        self.endpoint_addr().into()
    }

    /// Adds or refreshes out-of-band address information for a peer.
    pub fn add_ticket(&self, ticket: EndpointTicket) {
        self.inner
            .lookup
            .add_endpoint_info(ticket.endpoint_addr().clone());
    }

    /// Returns a point-in-time bounded-cardinality metrics snapshot.
    pub fn metrics(&self) -> MetricsSnapshot {
        self.inner.metrics.snapshot()
    }

    /// Gracefully closes this endpoint and all of its connections.
    pub async fn close(&self) {
        self.inner.metrics.set_online(false);
        self.inner.endpoint.close().await;
    }

    pub(crate) fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    pub(crate) fn config(&self) -> &IrohConfig {
        &self.inner.config
    }

    pub(crate) fn pool(&self) -> &ConnectionPool {
        &self.inner.pool
    }

    pub(crate) fn metrics_inner(&self) -> Arc<MetricsInner> {
        self.inner.metrics.clone()
    }
}

fn apply_bind_addr(builder: Builder, bind_addr: std::net::SocketAddr) -> Result<Builder, Error> {
    builder.bind_addr(bind_addr).map_err(|_| Error::Endpoint)
}

fn validate_config(config: &IrohConfig) -> Result<(), Error> {
    let limits = config.limits;
    if limits.max_connections == 0
        || limits.max_pooled_connections == 0
        || limits.max_connections_per_peer == 0
        || limits.max_streams == 0
        || limits.max_streams_per_connection == 0
        || limits.max_header_bytes < 8_192
        || limits.max_request_body_bytes == 0
        || limits.max_response_body_bytes == 0
        || config.timeouts.connect.is_zero()
        || config.timeouts.stream_open.is_zero()
        || config.timeouts.headers.is_zero()
        || config.timeouts.body_progress.is_zero()
        || config.timeouts.request.is_zero()
        || config.timeouts.connection_idle.is_zero()
        || config.timeouts.shutdown.is_zero()
    {
        return Err(Error::Endpoint);
    }
    if matches!(
        &config.discovery,
        DiscoveryMode::Custom { relay_urls } if relay_urls.is_empty()
    ) {
        return Err(Error::Endpoint);
    }
    Ok(())
}

impl fmt::Debug for IrohNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrohNode")
            .field("peer", &peer_fingerprint(self.endpoint_id()))
            .field("role", &self.inner.role)
            .field("discovery", &self.inner.config.discovery)
            .field("online", &self.metrics().endpoint_online)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_debug_redacts_addresses_and_tickets() {
        let relay: iroh::RelayUrl = "https://relay.example.invalid"
            .parse()
            .expect("valid relay URL");
        let config = IrohConfig {
            discovery: DiscoveryMode::Custom {
                relay_urls: vec![relay],
            },
            bind_addr: Some("127.0.0.1:9000".parse().expect("valid address")),
            ..IrohConfig::default()
        };
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("relay.example"));
        assert!(!rendered.contains("127.0.0.1"));
        assert!(rendered.contains("relay_count: 1"));
        assert!(rendered.contains("has_explicit_bind: true"));
    }
}
