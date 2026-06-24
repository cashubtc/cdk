//! # CDK Nostr Wallet Connect (NIP-47)
//!
//! A [NIP-47 Nostr Wallet Connect](https://github.com/nostr-protocol/nips/blob/master/47.md)
//! **wallet service**: the side that holds funds and answers commands sent by a
//! connected Nostr client (Damus, Amethyst, a website, …).
//!
//! The protocol wire types (connection URI, requests, responses, error codes)
//! come from [`nostr_sdk::nips::nip47`] and are re-exported here for
//! convenience. This crate adds:
//!
//! - [`NwcRequestHandler`]: the trait a wallet backend implements to service
//!   the supported commands.
//! - [`NwcService`]: owns the Nostr relay connection, advertises capabilities,
//!   authorizes and decrypts requests, dispatches to the handler, and publishes
//!   encrypted responses.
//!
//! The crate has no dependency on the `cdk` wallet crate; the Cashu-wallet
//! backed handler lives in `cdk::wallet::nwc`, keeping this layer reusable and
//! independently testable.
//!
//! ## Supported commands (first cut)
//!
//! `get_info`, `get_balance`, `make_invoice`, `pay_invoice`, `lookup_invoice`,
//! `list_transactions`. Any other command is answered with
//! [`ErrorCode::NotImplemented`](nostr_sdk::nips::nip47::ErrorCode::NotImplemented).
//!
//! All amounts in the NIP-47 protocol are denominated in **millisatoshis**.

#![warn(missing_docs)]

pub mod error;
pub mod handler;
pub mod service;

pub use error::{Error, Result};
pub use handler::NwcRequestHandler;
pub use service::{NwcService, NwcServiceConfig, SUPPORTED_METHODS};

// Re-export the NIP-47 protocol types so downstream crates depend on a single
// source of truth without pulling `nostr_sdk` paths into their signatures.
pub use nostr_sdk::nips::nip47;
