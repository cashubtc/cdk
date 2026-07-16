//! This crate is the base foundation to build things that can interact with the CDK (Cashu
//! Development Kit) and their internal crates.
//!
//! This is meant to contain the shared types, traits and common functions that are used across the
//! internal crates.

#![doc = include_str!("../README.md")]

pub mod task;

/// Authentication related types and utilities
///
/// Enabled by the `http` feature because OIDC auth helpers fetch discovery and
/// JWK documents through `cdk-http-client`.
#[cfg(feature = "http")]
pub mod auth;

/// Protocol version for gRPC Mint RPC communication
pub const MINT_RPC_PROTOCOL_VERSION: &str = "1.0.0";

/// Protocol version for gRPC Payment Processor communication
pub const PAYMENT_PROCESSOR_PROTOCOL_VERSION: &str = "3.0.0";

#[cfg(feature = "grpc")]
pub mod grpc;

pub mod common;
pub mod database;
pub mod error;
pub mod melt;
#[cfg(feature = "mint")]
pub mod mint;
pub mod mint_quote;
pub mod payjoin;
#[cfg(feature = "mint")]
pub mod payment;
pub mod pub_sub;
#[cfg(feature = "mint")]
pub mod state;
pub mod subscription;
#[cfg(feature = "wallet")]
pub mod wallet;
pub mod ws;

// re-exporting external crates
pub use bitcoin;
pub use cashu::amount::{self, Amount};
pub use cashu::lightning_invoice::{self, Bolt11Invoice};
pub use cashu::nuts::{self, *};
#[cfg(feature = "mint")]
pub use cashu::quote_id::{self, *};
pub use cashu::{dhke, ensure_cdk, mint_url, secret, util, SECP256K1};
/// Re-export `cdk-http-client` WebSocket client.
///
/// The `http` feature enables CDK common's HTTP-facing helpers and re-exports,
/// and selects the default `bitreq` backend. Add `cdk-http-client/reqwest`
/// elsewhere in the dependency graph to use `reqwest`; it takes precedence when
/// both backend features are enabled.
#[cfg(feature = "http")]
pub use cdk_http_client::ws as ws_client;
/// Re-export `cdk-http-client` types.
///
/// The `http` feature exposes these helpers and selects the default `bitreq`
/// backend. Applications that need SOCKS proxy or invalid-certificate support
/// can enable `cdk-http-client/reqwest`, which takes precedence over `bitreq`.
#[cfg(feature = "http")]
pub use cdk_http_client::{
    fetch, HttpClient, HttpClientBuilder, HttpError, RawResponse, RequestBuilder, Response,
};
// Re-export common types
pub use common::FinalizedMelt;
pub use error::Error;
pub use melt::{MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse};
pub use mint_quote::{MintQuoteRequest, MintQuoteResponse};
/// Re-export parking_lot for reuse
pub use parking_lot;
