//! CDK mint proto types

tonic::include_proto!("cdk_mint_management_rpc");
tonic::include_proto!("cdk_mint_reporting_rpc");

mod management;
mod reporting;
mod server;

pub use server::MintRPCServer;
