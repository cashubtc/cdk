#![doc = include_str!("../README.md")]

pub mod proto;

pub mod mint_rpc_cli;

pub use proto::*;

/// Type alias for the CdkMintClient that works with any tower service
pub type CdkMintClient<S> = cdk_mint_client::CdkMintClient<S>;
