//! Iroh transport bridge for CDK.
//!
//! This crate owns transport concerns only. Cashu HTTP routes are served
//! through their existing generic boundaries.

mod address;
mod client;
mod config;
mod error;
mod io;
mod metrics;
mod node;
mod pool;
pub mod protocol;
mod server;
mod transport;

pub use client::IrohClient;
pub use config::{DiscoveryMode, IrohConfig, IrohLimits, IrohTimeouts};
pub use error::Error;
pub use io::IrohStream;
pub use iroh::{EndpointAddr, EndpointId, RelayUrl, SecretKey};
pub use metrics::MetricsSnapshot;
pub use node::IrohNode;
pub use server::{IrohConnectionInfo, IrohServer};
pub use transport::IrohTransport;

/// Standard out-of-band bootstrap ticket for an Iroh endpoint.
pub type EndpointTicket = iroh_tickets::endpoint::EndpointTicket;
