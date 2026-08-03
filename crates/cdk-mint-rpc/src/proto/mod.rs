//! CDK mint proto types

tonic::include_proto!("cdk_mint_management_v1");

/// Keyset administration service
pub mod keyset {
    tonic::include_proto!("cdk_mint_keyset_v1");
}

/// Quote administration service
pub mod quote {
    tonic::include_proto!("cdk_mint_quote_v1");
}

mod server;

/// Protocol version for gRPC Mint RPC communication
pub use cdk_common::MINT_RPC_PROTOCOL_VERSION as PROTOCOL_VERSION;
pub use server::MintRPCServer;
