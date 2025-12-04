//! In memory signatory
//!
//! Implements the Signatory trait from cdk-common to manage the key in-process, to be included
//! inside the mint to be executed as a single process.
//!
//! Even if it is embedded in the same process, the keys are not accessible from the outside of this
//! module, all communication is done through the Signatory trait and the signatory manager.
#![deny(missing_docs)]
#![deny(warnings)]
#![deny(clippy::unwrap_used)]

#[cfg(feature = "grpc")]
mod proto;

#[cfg(feature = "grpc")]
pub use proto::{
    client::SignatoryRpcClient,
    server::{start_grpc_server, start_grpc_server_with_incoming, SignatoryLoader},
};

mod common;

pub mod db_signatory;
pub mod embedded;
pub mod signatory;
