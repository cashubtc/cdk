//! # CDK Nostr
//!
//! Nostr support for the Cashu Development Kit in a single crate:
//!
//! - [`keys`]: secret key generation/parsing and public key derivation.
//! - [`nip44`]: NIP-44 v2 encryption and decryption.
//! - [`inbox`]: a standing NIP-17 inbox listener that subscribes a set of
//!   relays for gift wraps addressed to a Nostr identity and delivers the
//!   unwrapped rumors to a [`NostrInboxListener`] callback.
//! - [`nwc`] (feature `nwc`): NIP-47 Nostr Wallet Connect wallet service.
//! - [`npubcash`] (feature `npubcash`): npub.cash API client.
//!
//! The crate intentionally has no dependency on the `cdk` wallet crate; the
//! Cashu-wallet glue lives in `cdk::wallet`, keeping this layer reusable and
//! independently testable.

#![warn(missing_docs)]

pub mod error;
pub mod inbox;
pub mod keys;
pub mod nip44;
#[cfg(feature = "npubcash")]
pub mod npubcash;
#[cfg(feature = "nwc")]
pub mod nwc;

pub use error::{Error, Result};
pub use inbox::{Nip17Event, NostrInbox, NostrInboxListener};
// Re-export the protocol and SDK crates so downstream crates use the same
// versions without adding direct dependencies.
pub use {nostr, nostr_sdk};
