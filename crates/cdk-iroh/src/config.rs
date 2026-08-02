//! Runtime configuration for Iroh endpoints and the HTTP bridge.

use std::{fmt, net::SocketAddr, time::Duration};

use iroh::RelayUrl;

use crate::{protocol, EndpointTicket};

/// Address discovery and relay policy for an Iroh endpoint.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// Use N0's production discovery and relay infrastructure.
    #[default]
    N0,
    /// Use only endpoint tickets supplied out of band and direct addresses.
    Static,
    /// Use operator-provided relay servers plus supplied endpoint tickets.
    Custom {
        /// Relay URLs used by this endpoint.
        relay_urls: Vec<RelayUrl>,
    },
}

impl fmt::Debug for DiscoveryMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::N0 => f.write_str("N0"),
            Self::Static => f.write_str("Static"),
            Self::Custom { relay_urls } => f
                .debug_struct("Custom")
                .field("relay_count", &relay_urls.len())
                .finish(),
        }
    }
}

impl DiscoveryMode {
    /// Uses only operator-provided relay servers and explicit endpoint tickets.
    pub fn custom(relay_urls: Vec<RelayUrl>) -> Self {
        Self::Custom { relay_urls }
    }
}

/// Timeouts applied at Iroh transport boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrohTimeouts {
    /// Maximum time spent establishing an authenticated connection.
    pub connect: Duration,
    /// Maximum time spent opening one request stream.
    pub stream_open: Duration,
    /// Maximum time spent waiting for request or response headers.
    pub headers: Duration,
    /// Maximum duration between successive request or response body frames.
    pub body_progress: Duration,
    /// Maximum duration of one non-WebSocket HTTP request.
    pub request: Duration,
    /// Maximum duration an admitted connection may remain without active streams.
    pub connection_idle: Duration,
    /// Maximum graceful shutdown drain duration.
    pub shutdown: Duration,
}

impl Default for IrohTimeouts {
    fn default() -> Self {
        Self {
            connect: protocol::CONNECT_TIMEOUT,
            stream_open: protocol::STREAM_OPEN_TIMEOUT,
            headers: protocol::HEADER_TIMEOUT,
            body_progress: protocol::BODY_PROGRESS_TIMEOUT,
            request: protocol::REQUEST_TIMEOUT,
            connection_idle: protocol::CONNECTION_IDLE_TIMEOUT,
            shutdown: protocol::SHUTDOWN_TIMEOUT,
        }
    }
}

/// Resource limits for Iroh HTTP clients and servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrohLimits {
    /// Maximum simultaneous incoming Iroh connections.
    pub max_connections: usize,
    /// Maximum distinct endpoint/ALPN entries retained by the outgoing pool.
    pub max_pooled_connections: usize,
    /// Maximum simultaneous incoming connections from one authenticated peer.
    pub max_connections_per_peer: usize,
    /// Maximum simultaneous HTTP streams across all connections.
    pub max_streams: usize,
    /// Maximum simultaneous HTTP streams on one connection.
    pub max_streams_per_connection: usize,
    /// Maximum HTTP request header buffer.
    pub max_header_bytes: usize,
    /// Maximum request body admitted before application-specific limits.
    pub max_request_body_bytes: usize,
    /// Default maximum response body collected by a client.
    pub max_response_body_bytes: usize,
}

impl Default for IrohLimits {
    fn default() -> Self {
        Self {
            max_connections: 1_024,
            max_pooled_connections: 1_024,
            max_connections_per_peer: 8,
            max_streams: 4_096,
            max_streams_per_connection: 256,
            max_header_bytes: protocol::MAX_HEADER_BYTES,
            max_request_body_bytes: protocol::MAX_REQUEST_BODY_BYTES,
            max_response_body_bytes: protocol::MAX_RESPONSE_BODY_BYTES,
        }
    }
}

/// Complete endpoint and bridge configuration.
#[derive(Clone, Default)]
pub struct IrohConfig {
    /// Discovery and relay policy.
    pub discovery: DiscoveryMode,
    /// Initial out-of-band endpoint tickets.
    pub static_tickets: Vec<EndpointTicket>,
    /// Optional explicit UDP bind address.
    pub bind_addr: Option<SocketAddr>,
    /// Transport timeouts.
    pub timeouts: IrohTimeouts,
    /// Admission and body limits.
    pub limits: IrohLimits,
}

impl IrohConfig {
    /// Configuration for a local or ticket-only endpoint with no public relay dependency.
    pub fn static_only() -> Self {
        Self {
            discovery: DiscoveryMode::Static,
            ..Self::default()
        }
    }

    /// Adds an initial endpoint ticket.
    pub fn with_ticket(mut self, ticket: EndpointTicket) -> Self {
        self.static_tickets.push(ticket);
        self
    }

    /// Sets an explicit UDP bind address.
    pub fn with_bind_addr(mut self, bind_addr: SocketAddr) -> Self {
        self.bind_addr = Some(bind_addr);
        self
    }
}

impl fmt::Debug for IrohConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IrohConfig")
            .field("discovery", &self.discovery)
            .field("static_ticket_count", &self.static_tickets.len())
            .field("has_explicit_bind", &self.bind_addr.is_some())
            .field("timeouts", &self.timeouts)
            .field("limits", &self.limits)
            .finish()
    }
}
