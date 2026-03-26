#![doc = include_str!("../README.md")]

pub mod proto;

pub mod mint_rpc_cli;

pub use proto::*;

/// Type alias for the CdkMintClient that works with any tower service
pub type CdkMintClient<S> = cdk_mint_client::CdkMintClient<S>;

/// Type alias for CdkMintClient with the version header interceptor over a Channel
pub type InterceptedCdkMintClient = cdk_mint_client::CdkMintClient<
    tonic::codegen::InterceptedService<
        tonic::transport::Channel,
        cdk_common::grpc::VersionInterceptor,
    >,
>;
