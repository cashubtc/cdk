//! On-chain send saga for batched Bitcoin transactions
//!
//! This module implements the send-side saga for on-chain payments. The design
//! persists payment intents before transaction construction and supports
//! immediate and delayed batching through a single flow.

pub(crate) mod batch_transaction;
pub(crate) mod payment_intent;
pub(crate) mod service;
