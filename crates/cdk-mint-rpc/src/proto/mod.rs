//! CDK mint proto types

tonic::include_proto!("cdk_mint_management_rpc");
tonic::include_proto!("cdk_mint_data_rpc");

mod server;

pub use server::MintRPCServer;
