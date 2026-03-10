//! CDK mint proto types

tonic::include_proto!("cdk_mint_management_v1");

mod server;

/// Protocol version for gRPC Mint RPC communication
pub use cdk_common::MINT_RPC_PROTOCOL_VERSION as PROTOCOL_VERSION;
pub use server::MintRPCServer;
