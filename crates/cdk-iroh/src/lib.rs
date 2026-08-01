//! Iroh transport bridge for CDK.
//!
//! This crate owns transport concerns only. Cashu HTTP routes and federation
//! state machines are served through their existing generic boundaries.

mod io;
pub mod protocol;

pub use io::IrohStream;

/// Standard out-of-band bootstrap ticket for an Iroh endpoint.
pub type EndpointTicket = iroh_tickets::endpoint::EndpointTicket;
