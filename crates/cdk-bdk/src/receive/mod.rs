//! Receive saga types and modules
//!
//! Implements the on-chain receive saga for tracking incoming payments.
//! Each address created via `create_incoming_payment_request()` is stored
//! as a `ReceiveAddressRecord`, and each incoming UTXO is tracked
//! independently as a `ReceiveIntent`.
//!
//! ## Architecture
//!
//! The receive flow uses two durable storage concepts:
//!
//! 1. **Tracked receive address index** -- a durable `address -> quote_id`
//!    mapping used by wallet scans to associate observed UTXOs with mint
//!    quotes.
//! 2. **`ReceiveIntent`** -- one record per detected incoming UTXO. This is
//!    created when block processing sees funds sent to a tracked address.
//!
//! ## Typestate Flow
//!
//! `ReceiveIntent`: `Detected` -> finalized (tombstone)
//!
//! Unlike the send saga, receive is observational. There is no compensation
//! step because the wallet does not need to roll back any outbound side effects.
//! Once a detected UTXO reaches the required confirmation depth, the active
//! intent is finalized into a tombstone so historical status queries can still
//! return the payment after the active record is removed.

pub(crate) mod receive_intent;
pub(crate) mod service;
