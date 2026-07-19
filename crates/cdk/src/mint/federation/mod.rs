//! Federation mode support for CDK mints.
//!
//! This module applies accepted federation operations to local mint state,
//! validates aggregate BLS keysets, and produces member signature shares while
//! preserving standalone mint behavior outside federation mode.

mod address_allocation;
mod chain_observation;
mod projection;
mod transaction_policy;
mod transaction_state;

include!("configuration_and_admission.rs");
include!("application.rs");
include!("helpers.rs");
