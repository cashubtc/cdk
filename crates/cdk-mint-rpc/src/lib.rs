#![doc = include_str!("../README.md")]

pub mod proto;

pub mod mint_rpc_cli;

pub use proto::*;

/// Type alias for the CdkMintManagementClient that works with any tower service
pub type CdkMintManagementClient<S> = cdk_mint_management_client::CdkMintManagementClient<S>;

/// Type alias for the CdkMintReportingClient that works with any tower service
pub type CdkMintReportingClient<S> = cdk_mint_reporting_client::CdkMintReportingClient<S>;
